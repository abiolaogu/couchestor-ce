# CoucheStor Installation Guide for Nutanix

This guide provides comprehensive instructions for deploying CoucheStor on Nutanix Acropolis Hyperconverged Infrastructure (AHV) with Nutanix Kubernetes Engine (NKE) or self-managed Kubernetes.

## Table of Contents

1. [Prerequisites](#prerequisites)
2. [Architecture Overview](#architecture-overview)
3. [Nutanix Environment Preparation](#nutanix-environment-preparation)
4. [Kubernetes Cluster Deployment](#kubernetes-cluster-deployment)
5. [Storage Configuration](#storage-configuration)
6. [Network Configuration](#network-configuration)
7. [Deploying CoucheStor](#deploying-couchestor)
8. [Nutanix CSI Integration](#nutanix-csi-integration)
9. [Prism Central Integration](#prism-central-integration)
10. [Files and Objects Integration](#files-and-objects-integration)
11. [Monitoring Integration](#monitoring-integration)
12. [Disaster Recovery](#disaster-recovery)
13. [Verification and Testing](#verification-and-testing)
14. [Troubleshooting](#troubleshooting)
15. [Appendix](#appendix)

---

## Prerequisites

### Nutanix Requirements

| Component | Version | Required |
|-----------|---------|----------|
| AOS | 6.5+ | Yes |
| Prism Central | pc.2023.3+ | Recommended |
| AHV | 20220304+ | Yes |
| NKE | 2.8+ | Optional |
| Nutanix CSI Driver | 2.6+ | Yes |
| Nutanix Files | 4.2+ | Optional |
| Nutanix Objects | 3.5+ | Optional |

### Hardware Requirements

| Resource | Minimum | Recommended |
|----------|---------|-------------|
| Nodes | 3 | 4+ |
| CPU Cores | 32 per node | 64+ per node |
| RAM | 128 GB per node | 256 GB+ per node |
| NVMe Tier | 2 TB per node | 4 TB+ per node |
| SSD Tier | 4 TB per node | 8 TB+ per node |
| HDD Tier (optional) | - | 20 TB+ per node |
| Network | 10 GbE | 25/100 GbE |

### Software Requirements

- kubectl v1.27+
- Helm v3.12+
- Nutanix kubectl plugin (nkp)
- Prism Central API access

### Network Requirements

- Management network for Prism access
- VM network(s) for Kubernetes nodes
- iSCSI network for Volumes CSI (dedicated recommended)
- Minimum MTU: 1500 (9000 recommended for storage)

---

## Architecture Overview

### CoucheStor on Nutanix Topology

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                           Nutanix Cluster                                        │
│                                                                                  │
│  ┌──────────────────────────────────────────────────────────────────────────┐   │
│  │                    Prism Central / Prism Element                          │   │
│  │  ┌────────────────┐  ┌────────────────┐  ┌────────────────────────────┐  │   │
│  │  │  Categories    │  │  Protection    │  │  Calm / NCM                │  │   │
│  │  │  (Tiering)     │  │  Policies      │  │  (Automation)              │  │   │
│  │  └────────────────┘  └────────────────┘  └────────────────────────────┘  │   │
│  └──────────────────────────────────────────────────────────────────────────┘   │
│                                     │                                            │
│  ┌──────────────────────────────────▼──────────────────────────────────────┐    │
│  │                    NKE / Kubernetes Cluster                              │    │
│  │                                                                          │    │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                   │    │
│  │  │  Master VM   │  │  Master VM   │  │  Master VM   │  Control Plane    │    │
│  │  └──────────────┘  └──────────────┘  └──────────────┘                   │    │
│  │                                                                          │    │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                   │    │
│  │  │  Worker VM   │  │  Worker VM   │  │  Worker VM   │                   │    │
│  │  │┌────────────┐│  │┌────────────┐│  │┌────────────┐│                   │    │
│  │  ││ CoucheStor ││  ││ CoucheStor ││  ││ CoucheStor ││                   │    │
│  │  ││   Agent    ││  ││   Agent    ││  ││   Agent    ││                   │    │
│  │  │└────────────┘│  │└────────────┘│  │└────────────┘│                   │    │
│  │  └──────────────┘  └──────────────┘  └──────────────┘                   │    │
│  │              │              │              │                              │    │
│  └──────────────┼──────────────┼──────────────┼──────────────────────────────┘   │
│                 │              │              │                                   │
│  ┌──────────────▼──────────────▼──────────────▼──────────────────────────────┐  │
│  │                     Nutanix Distributed Storage Fabric                     │  │
│  │                                                                            │  │
│  │  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────────┐     │  │
│  │  │   NVMe Tier      │  │   SSD Tier       │  │   HDD Tier           │     │  │
│  │  │  (Hot Storage)   │  │  (Warm Storage)  │  │  (Cold Storage)      │     │  │
│  │  │  Storage Pool    │  │  Storage Pool    │  │  Storage Pool        │     │  │
│  │  └──────────────────┘  └──────────────────┘  └──────────────────────┘     │  │
│  │                                                                            │  │
│  │  ┌──────────────────────────────────────────────────────────────────────┐ │  │
│  │  │              Nutanix Volumes / Files / Objects                        │ │  │
│  │  └──────────────────────────────────────────────────────────────────────┘ │  │
│  └────────────────────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────────────────┘
```

### Integration Points

1. **Nutanix CSI Driver**: Block and file volumes via Nutanix Volumes and Files
2. **Prism Central API**: Category-based tiering, protection policies
3. **Nutanix Objects**: S3-compatible object storage for cold tier
4. **X-Play**: Automated actions based on storage metrics

---

## Nutanix Environment Preparation

### 1. Configure Prism Central

```bash
# Access Prism Central and enable the following:
# - Kubernetes Management
# - Calm (optional, for automation)
# - Flow (for microsegmentation)
```

### 2. Create Storage Containers

Access Prism Element → Storage → Storage Containers:

```bash
# Via ncli or Prism API

# Create Hot Tier Container (NVMe)
ncli container create name=couchestor-hot-tier \
  rf=2 \
  compression-enabled=true \
  compression-delay-in-secs=0 \
  on-disk-dedup=OFF \
  erasure-code=OFF

# Create Warm Tier Container (SSD)
ncli container create name=couchestor-warm-tier \
  rf=2 \
  compression-enabled=true \
  compression-delay-in-secs=3600 \
  on-disk-dedup=POST_PROCESS \
  erasure-code=OFF

# Create Cold Tier Container (HDD with EC)
ncli container create name=couchestor-cold-tier \
  rf=2 \
  compression-enabled=true \
  compression-delay-in-secs=0 \
  on-disk-dedup=POST_PROCESS \
  erasure-code=ON
```

### 3. Create Categories for Tiering

In Prism Central → Administration → Categories:

```python
# Via Prism Central API
import requests

pc_url = "https://prism-central.example.com:9440"
headers = {"Content-Type": "application/json"}
auth = ("admin", "password")

# Create CoucheStor category
category = {
    "api_version": "3.1.0",
    "metadata": {"kind": "category"},
    "spec": {
        "name": "CoucheStor",
        "description": "CoucheStor storage tiering",
        "capabilities": {"cardinality": 64}
    }
}

# Create tier values
for tier in ["Hot", "Warm", "Cold"]:
    value = {
        "api_version": "3.1.0",
        "metadata": {"kind": "category"},
        "spec": {
            "name": "CoucheStor",
            "value": tier
        }
    }
    requests.put(f"{pc_url}/api/nutanix/v3/categories/CoucheStor/{tier}",
                 json=value, auth=auth, headers=headers, verify=False)
```

### 4. Create Volume Groups for Tiers

```bash
# Via ncli or Prism API

# Hot tier Volume Group
ncli volume-group create name=couchestor-hot-vg \
  flash-mode=true \
  load-balance-vm-attachments=true

# Warm tier Volume Group
ncli volume-group create name=couchestor-warm-vg \
  flash-mode=false \
  load-balance-vm-attachments=true

# Cold tier Volume Group
ncli volume-group create name=couchestor-cold-vg \
  flash-mode=false \
  load-balance-vm-attachments=true
```

---

## Kubernetes Cluster Deployment

### Option 1: Nutanix Kubernetes Engine (NKE) - Recommended

#### 1. Create Kubernetes Cluster via Prism Central

```bash
# Using nkp CLI
nkp cluster create \
  --name couchestor-cluster \
  --kubernetes-version 1.28.5 \
  --control-plane-count 3 \
  --worker-count 3 \
  --control-plane-vcpus 4 \
  --control-plane-memory 16384 \
  --control-plane-disk 120 \
  --worker-vcpus 8 \
  --worker-memory 32768 \
  --worker-disk 200 \
  --network-name VM-Network \
  --cluster-name nutanix-cluster \
  --prism-central-endpoint prism-central.example.com

# Wait for cluster creation
nkp cluster status couchestor-cluster

# Get kubeconfig
nkp cluster get-credentials couchestor-cluster
export KUBECONFIG=~/.kube/couchestor-cluster.kubeconfig
```

#### 2. Configure NKE Cluster for CoucheStor

```bash
# Enable privileged containers (required for agents)
kubectl patch deployment -n kube-system nke-cluster-autoscaler \
  --type=json -p='[{"op":"add","path":"/spec/template/spec/containers/0/args/-","value":"--expendable-pods-priority-cutoff=-10"}]'

# Verify cluster
kubectl get nodes -o wide
```

### Option 2: Manual Kubernetes Deployment on AHV

#### 1. Create VMs via Prism

```bash
# Via Prism API or Calm blueprint

# Create master VMs
for i in 1 2 3; do
  ncli vm create name=k8s-master-$i \
    memory=16G \
    num-vcpus=4 \
    num-cores-per-vcpu=1
done

# Create worker VMs
for i in 1 2 3; do
  ncli vm create name=k8s-worker-$i \
    memory=32G \
    num-vcpus=8 \
    num-cores-per-vcpu=1
done
```

#### 2. Install Kubernetes with kubeadm

```bash
# On all nodes - run the following
cat > /etc/modules-load.d/k8s.conf << EOF
overlay
br_netfilter
EOF

modprobe overlay
modprobe br_netfilter

cat > /etc/sysctl.d/k8s.conf << EOF
net.bridge.bridge-nf-call-iptables = 1
net.bridge.bridge-nf-call-ip6tables = 1
net.ipv4.ip_forward = 1
EOF

sysctl --system

# Install containerd and kubernetes packages
# ... (standard kubeadm installation)

# Initialize cluster on first master
kubeadm init --control-plane-endpoint "k8s-api.example.com:6443" \
  --pod-network-cidr=10.244.0.0/16 \
  --upload-certs
```

---

## Storage Configuration

### 1. Install Nutanix CSI Driver

```bash
# Add Nutanix Helm repository
helm repo add nutanix https://nutanix.github.io/helm/
helm repo update

# Create namespace
kubectl create namespace ntnx-system

# Create secret for Prism credentials
kubectl create secret generic ntnx-secret \
  --namespace ntnx-system \
  --from-literal=key='prism-central.example.com:9440:admin:password'
```

```yaml
# nutanix-csi-values.yaml
createSecret: false
secretName: ntnx-secret

prismEndpoint: prism-central.example.com

storageContainer: couchestor-hot-tier
fsType: ext4

volumeClass: true
volumeClassName: ntnx-volumes

fileClass: true
fileClassName: ntnx-files
fileServerName: files.example.com

dynamicFileClass: true
dynamicFileClassName: ntnx-dynamic-files

defaultStorageClass: ntnx-volumes

kubeletDir: /var/lib/kubelet

node:
  nodeSelector:
    kubernetes.io/os: linux

controller:
  replicas: 2
  nodeSelector:
    node-role.kubernetes.io/control-plane: ""
```

```bash
helm install nutanix-csi nutanix/nutanix-csi-storage \
  --namespace ntnx-system \
  --values nutanix-csi-values.yaml
```

### 2. Create Storage Classes for Tiers

```yaml
# storage-classes-nutanix.yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-hot
  annotations:
    storageclass.kubernetes.io/is-default-class: "false"
provisioner: csi.nutanix.com
parameters:
  csi.storage.k8s.io/provisioner-secret-name: ntnx-secret
  csi.storage.k8s.io/provisioner-secret-namespace: ntnx-system
  csi.storage.k8s.io/node-publish-secret-name: ntnx-secret
  csi.storage.k8s.io/node-publish-secret-namespace: ntnx-system
  csi.storage.k8s.io/controller-expand-secret-name: ntnx-secret
  csi.storage.k8s.io/controller-expand-secret-namespace: ntnx-system
  storageContainer: couchestor-hot-tier
  storageType: NutanixVolumes
  flashMode: "ENABLED"
  isSegmentedIscsiNetwork: "false"
reclaimPolicy: Delete
allowVolumeExpansion: true
volumeBindingMode: WaitForFirstConsumer
---
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-warm
provisioner: csi.nutanix.com
parameters:
  csi.storage.k8s.io/provisioner-secret-name: ntnx-secret
  csi.storage.k8s.io/provisioner-secret-namespace: ntnx-system
  csi.storage.k8s.io/node-publish-secret-name: ntnx-secret
  csi.storage.k8s.io/node-publish-secret-namespace: ntnx-system
  csi.storage.k8s.io/controller-expand-secret-name: ntnx-secret
  csi.storage.k8s.io/controller-expand-secret-namespace: ntnx-system
  storageContainer: couchestor-warm-tier
  storageType: NutanixVolumes
  flashMode: "DISABLED"
reclaimPolicy: Delete
allowVolumeExpansion: true
volumeBindingMode: WaitForFirstConsumer
---
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-cold
provisioner: csi.nutanix.com
parameters:
  csi.storage.k8s.io/provisioner-secret-name: ntnx-secret
  csi.storage.k8s.io/provisioner-secret-namespace: ntnx-system
  csi.storage.k8s.io/node-publish-secret-name: ntnx-secret
  csi.storage.k8s.io/node-publish-secret-namespace: ntnx-system
  csi.storage.k8s.io/controller-expand-secret-name: ntnx-secret
  csi.storage.k8s.io/controller-expand-secret-namespace: ntnx-system
  storageContainer: couchestor-cold-tier
  storageType: NutanixVolumes
  flashMode: "DISABLED"
reclaimPolicy: Delete
allowVolumeExpansion: true
volumeBindingMode: WaitForFirstConsumer
```

```bash
kubectl apply -f storage-classes-nutanix.yaml
```

### 3. Configure iSCSI Network (Recommended)

```yaml
# iscsi-network-config.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: iscsi-network-config
  namespace: ntnx-system
data:
  iscsi_network_segment_name: "iscsi-segment"
  iscsi_network_enabled: "true"
```

---

## Network Configuration

### 1. Create Microsegmentation Policy (Flow)

In Prism Central → Network & Security → Security Policies:

```yaml
# flow-policy.yaml (conceptual)
name: couchestor-security-policy
type: Application
rules:
  # Allow CoucheStor controller to communicate with agents
  - source:
      category:
        key: AppType
        value: CoucheStor-Controller
    destination:
      category:
        key: AppType
        value: CoucheStor-Agent
    ports:
      - protocol: TCP
        start: 8080
        end: 8081

  # Allow iSCSI traffic
  - source:
      category:
        key: AppType
        value: CoucheStor-Agent
    destination:
      category:
        key: AppType
        value: Storage
    ports:
      - protocol: TCP
        start: 3260
        end: 3260
```

### 2. Configure Kubernetes Network Policies

```yaml
# network-policies-nutanix.yaml
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
        - namespaceSelector: {}
      ports:
        - protocol: TCP
          port: 8080
        - protocol: TCP
          port: 8081
  egress:
    - to:
        - ipBlock:
            cidr: 0.0.0.0/0
      ports:
        - protocol: TCP
          port: 9440   # Prism Central
        - protocol: TCP
          port: 3260   # iSCSI
        - protocol: TCP
          port: 9090   # Prometheus
```

```bash
kubectl apply -f network-policies-nutanix.yaml
```

---

## Deploying CoucheStor

### 1. Create Namespace and Secrets

```bash
# Create namespace
kubectl create namespace couchestor-system

# Create Nutanix credentials secret
kubectl create secret generic nutanix-credentials \
  --namespace couchestor-system \
  --from-literal=PRISM_CENTRAL_URL=https://prism-central.example.com:9440 \
  --from-literal=PRISM_USERNAME=admin \
  --from-literal=PRISM_PASSWORD=your-password \
  --from-literal=PRISM_CLUSTER_UUID=$(ncli cluster info | grep "Cluster Id" | awk '{print $4}')
```

### 2. Install CRDs

```bash
git clone https://github.com/abiolaogu/couchestor.git
cd couchestor
kubectl apply -f deploy/crds/
```

### 3. Configure CoucheStor for Nutanix

```yaml
# couchestor-config-nutanix.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: couchestor-config
  namespace: couchestor-system
data:
  config.yaml: |
    cloud_provider: nutanix

    nutanix:
      prism_central_url: "https://prism-central.example.com:9440"
      cluster_uuid: "<cluster-uuid>"
      credential_ref:
        secret_name: nutanix-credentials
        namespace: couchestor-system

      # Use Nutanix Categories for tiering
      use_categories: true
      category_key: "CoucheStor"

      # Flash mode settings
      hot_tier_flash_mode: true
      warm_tier_flash_mode: false
      cold_tier_flash_mode: false

    prometheus:
      endpoint: "http://prometheus-server.monitoring.svc:9090"
      query_timeout: 30s

    tiers:
      hot:
        storage_class: "couchestor-hot"
        nutanix_container: "couchestor-hot-tier"
        category_value: "Hot"
        iops_threshold: 100
        min_capacity_gb: 100
      warm:
        storage_class: "couchestor-warm"
        nutanix_container: "couchestor-warm-tier"
        category_value: "Warm"
        iops_threshold: 10
        min_capacity_gb: 500
      cold:
        storage_class: "couchestor-cold"
        nutanix_container: "couchestor-cold-tier"
        category_value: "Cold"
        iops_threshold: 0
        min_capacity_gb: 2000

    erasure_coding:
      enabled: true
      data_shards: 4
      parity_shards: 2
      stripe_size_mb: 64
      # Use Nutanix native EC for cold tier
      use_nutanix_ec: true

    migration:
      cooldown_period: 1h
      batch_size: 5
      concurrent_migrations: 2
      # Nutanix-specific: Use storage vMotion
      use_storage_vmotion: true

    cache:
      l1_size_mb: 1024
      l2_size_mb: 10240
      compression: zstd
      prefetch_enabled: true
```

```bash
kubectl apply -f couchestor-config-nutanix.yaml
```

### 4. Deploy CoucheStor Controller

```yaml
# couchestor-deployment-nutanix.yaml
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
      annotations:
        nutanix.com/category: "AppType:CoucheStor-Controller"
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
            - --cloud-provider=nutanix
          ports:
            - containerPort: 8080
              name: metrics
            - containerPort: 8081
              name: health
          env:
            - name: PRISM_CENTRAL_URL
              valueFrom:
                secretKeyRef:
                  name: nutanix-credentials
                  key: PRISM_CENTRAL_URL
            - name: PRISM_USERNAME
              valueFrom:
                secretKeyRef:
                  name: nutanix-credentials
                  key: PRISM_USERNAME
            - name: PRISM_PASSWORD
              valueFrom:
                secretKeyRef:
                  name: nutanix-credentials
                  key: PRISM_PASSWORD
            - name: PRISM_CLUSTER_UUID
              valueFrom:
                secretKeyRef:
                  name: nutanix-credentials
                  key: PRISM_CLUSTER_UUID
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
```

```bash
kubectl apply -f couchestor-deployment-nutanix.yaml
```

### 5. Deploy CoucheStor Agent

```yaml
# couchestor-agent-nutanix.yaml
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
      annotations:
        nutanix.com/category: "AppType:CoucheStor-Agent"
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
            - --cloud-provider=nutanix
          env:
            - name: NODE_NAME
              valueFrom:
                fieldRef:
                  fieldPath: spec.nodeName
            - name: PRISM_CENTRAL_URL
              valueFrom:
                secretKeyRef:
                  name: nutanix-credentials
                  key: PRISM_CENTRAL_URL
            - name: PRISM_USERNAME
              valueFrom:
                secretKeyRef:
                  name: nutanix-credentials
                  key: PRISM_USERNAME
            - name: PRISM_PASSWORD
              valueFrom:
                secretKeyRef:
                  name: nutanix-credentials
                  key: PRISM_PASSWORD
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
            - name: iscsi
              mountPath: /etc/iscsi
      volumes:
        - name: dev
          hostPath:
            path: /dev
        - name: sys
          hostPath:
            path: /sys
        - name: iscsi
          hostPath:
            path: /etc/iscsi
      tolerations:
        - operator: Exists
```

```bash
kubectl apply -f couchestor-agent-nutanix.yaml
```

### 6. Create RBAC

```yaml
# rbac-nutanix.yaml
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
    verbs: ["get", "list", "watch", "update", "patch", "delete"]
  - apiGroups: [""]
    resources: ["events"]
    verbs: ["create", "patch"]
  - apiGroups: [""]
    resources: ["nodes"]
    verbs: ["get", "list", "watch"]
  - apiGroups: ["storage.k8s.io"]
    resources: ["storageclasses", "volumeattachments"]
    verbs: ["get", "list", "watch"]
  - apiGroups: ["couchestor.io"]
    resources: ["*"]
    verbs: ["*"]
  - apiGroups: ["coordination.k8s.io"]
    resources: ["leases"]
    verbs: ["get", "list", "watch", "create", "update", "patch", "delete"]
  - apiGroups: ["snapshot.storage.k8s.io"]
    resources: ["volumesnapshots", "volumesnapshotcontents", "volumesnapshotclasses"]
    verbs: ["get", "list", "watch", "create", "delete"]
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
kubectl apply -f rbac-nutanix.yaml
```

---

## Nutanix CSI Integration

### Understanding Nutanix Volume Migrations

CoucheStor leverages Nutanix's native storage features:

1. **Storage vMotion**: Live migration between containers
2. **Categories**: Automatic tiering metadata
3. **Native Erasure Coding**: Nutanix AOS EC for cold tier

### Storage Policy with Nutanix Features

```yaml
# storage-policy-nutanix.yaml
apiVersion: couchestor.io/v1alpha1
kind: StoragePolicy
metadata:
  name: nutanix-tiered-policy
  namespace: couchestor-system
spec:
  cloudProvider: nutanix

  nutanixConfig:
    # Use Nutanix native storage tiering
    useNativeStoragePolicy: true

    # Category-based management
    categoryKey: "CoucheStor"

    # Storage container mapping
    containers:
      hot: "couchestor-hot-tier"
      warm: "couchestor-warm-tier"
      cold: "couchestor-cold-tier"

    # Flash mode settings
    flashModeHot: true
    flashModeWarm: false
    flashModeCold: false

  tiers:
    - name: hot
      storageClass: couchestor-hot
      categoryValue: "Hot"
      iopsThreshold: 100
      retentionDays: 7
    - name: warm
      storageClass: couchestor-warm
      categoryValue: "Warm"
      iopsThreshold: 10
      retentionDays: 30
    - name: cold
      storageClass: couchestor-cold
      categoryValue: "Cold"
      iopsThreshold: 0
      retentionDays: 365

  migration:
    strategy: storage-vmotion  # nutanix-native, clone-swap, snapshot-restore
    schedule: "0 2 * * *"
    cooldownPeriod: 24h
    maxConcurrent: 3

    nutanixOptions:
      # Use Nutanix VAAI for acceleration
      useVaai: true
      # Maintain VM running during migration
      liveDataMigration: true
      # Post-migration container cleanup
      cleanupSourceAfterMigration: true

  erasureCoding:
    enabled: true
    coldTierOnly: true
    # Use Nutanix native EC when available
    useNutanixEC: true
    # Fallback to CoucheStor EC if Nutanix EC not available
    fallbackConfig:
      dataShards: 4
      parityShards: 2
```

```bash
kubectl apply -f storage-policy-nutanix.yaml
```

---

## Prism Central Integration

### 1. Configure X-Play Automation

Create automated playbooks in Prism Central → Operations → Playbooks:

```yaml
# x-play-playbook.yaml (conceptual)
name: CoucheStor Volume Tiering Alert
trigger:
  type: Category
  category: "CoucheStor"
  condition: Value changes

actions:
  - type: Email
    recipients:
      - storage-team@example.com
    subject: "CoucheStor Volume Tier Change"
    body: "Volume {{entity.name}} moved to {{entity.category.CoucheStor}}"

  - type: REST API
    url: "http://couchestor-controller.couchestor-system.svc:8080/webhook"
    method: POST
    body: |
      {
        "event": "tier_change",
        "volume": "{{entity.name}}",
        "tier": "{{entity.category.CoucheStor}}"
      }
```

### 2. Create Protection Policies

```yaml
# protection-policy.yaml (via Prism API)
{
  "spec": {
    "name": "couchestor-protection",
    "description": "Protection policy for CoucheStor volumes",
    "resources": {
      "ordered_availability_zone_list": [
        {
          "availability_zone_url": "az://local",
          "replication_type": "SYNC"
        }
      ],
      "category_filter": {
        "kind_list": ["vm_disk"],
        "type": "CATEGORIES_MATCH_ANY",
        "params": {
          "CoucheStor": ["Hot", "Warm"]
        }
      },
      "primary_location_index": 0,
      "start_time": "00:00",
      "rpo_secs": 3600,
      "snapshot_type": "APPLICATION_CONSISTENT",
      "local_retention_policy": {
        "num_snapshots": 24
      }
    }
  }
}
```

### 3. Integrate with Nutanix APIs

```yaml
# nutanix-api-integration.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: nutanix-api-config
  namespace: couchestor-system
data:
  api-config.yaml: |
    prism_central:
      api_version: "v3"
      endpoints:
        volumes: "/api/nutanix/v3/volumes"
        vdisks: "/api/nutanix/v3/virtual_disks"
        categories: "/api/nutanix/v3/categories"
        protection_rules: "/api/nutanix/v3/protection_rules"
        recovery_plans: "/api/nutanix/v3/recovery_plans"

    prism_element:
      api_version: "v2"
      endpoints:
        containers: "/api/nutanix/v2/storage_containers"
        vgs: "/api/nutanix/v2/volume_groups"
        hosts: "/api/nutanix/v2/hosts"
```

---

## Files and Objects Integration

### Nutanix Files Integration (Shared Storage)

```yaml
# files-storage-class.yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-shared
provisioner: csi.nutanix.com
parameters:
  csi.storage.k8s.io/provisioner-secret-name: ntnx-secret
  csi.storage.k8s.io/provisioner-secret-namespace: ntnx-system
  nfsServerName: files.example.com
  storageType: NutanixFiles
reclaimPolicy: Delete
allowVolumeExpansion: true
```

### Nutanix Objects Integration (S3 Cold Tier)

```yaml
# objects-config.yaml
apiVersion: v1
kind: Secret
metadata:
  name: nutanix-objects-credentials
  namespace: couchestor-system
type: Opaque
stringData:
  AWS_ACCESS_KEY_ID: "<objects-access-key>"
  AWS_SECRET_ACCESS_KEY: "<objects-secret-key>"
  AWS_ENDPOINT_URL: "https://objects.example.com"
  AWS_REGION: "us-east-1"
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: nutanix-objects-config
  namespace: couchestor-system
data:
  objects.yaml: |
    endpoint: "https://objects.example.com"
    bucket: "couchestor-cold-tier"
    region: "us-east-1"
    use_path_style: true

    # Lifecycle policies
    lifecycle:
      intelligent_tiering: true
      transition_to_glacier_days: 90
      expiration_days: 365
```

---

## Monitoring Integration

### 1. Deploy Prometheus via NKE Add-on

```bash
# Enable monitoring add-on in NKE
nkp cluster addon enable couchestor-cluster --addon prometheus
```

Or deploy manually:

```bash
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm install prometheus prometheus-community/kube-prometheus-stack \
  --namespace monitoring \
  --create-namespace
```

### 2. Configure ServiceMonitor

```yaml
# servicemonitor-nutanix.yaml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: couchestor
  namespace: monitoring
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
---
# Nutanix-specific metrics
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: nutanix-csi
  namespace: monitoring
spec:
  selector:
    matchLabels:
      app: nutanix-csi
  namespaceSelector:
    matchNames:
      - ntnx-system
  endpoints:
    - port: metrics
      interval: 30s
```

### 3. Import Grafana Dashboards

```bash
# Port-forward to Grafana
kubectl -n monitoring port-forward svc/prometheus-grafana 3000:80

# Import CoucheStor dashboard (Dashboard ID: xxxxx)
# Import Nutanix CSI dashboard
```

### 4. Alerting Rules

```yaml
# alerting-rules-nutanix.yaml
apiVersion: monitoring.coreos.com/v1
kind: PrometheusRule
metadata:
  name: couchestor-alerts
  namespace: monitoring
spec:
  groups:
    - name: couchestor-nutanix
      rules:
        - alert: CouchestorNutanixConnectivityLost
          expr: absent(couchestor_nutanix_api_healthy == 1)
          for: 5m
          labels:
            severity: critical
          annotations:
            summary: "Lost connectivity to Nutanix Prism"

        - alert: NutanixContainerCapacityLow
          expr: nutanix_container_free_bytes / nutanix_container_total_bytes < 0.1
          for: 30m
          labels:
            severity: warning
          annotations:
            summary: "Nutanix container {{ $labels.container }} low on space"

        - alert: CouchestorMigrationStalled
          expr: increase(couchestor_migrations_completed_total[1h]) == 0 and couchestor_migrations_pending > 0
          for: 2h
          labels:
            severity: warning
          annotations:
            summary: "CoucheStor migrations appear stalled"
```

---

## Disaster Recovery

### 1. Configure Nutanix DR

```yaml
# dr-config.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: couchestor-dr-config
  namespace: couchestor-system
data:
  dr.yaml: |
    disaster_recovery:
      enabled: true

      # Nutanix Leap integration
      leap:
        enabled: true
        recovery_plan: "couchestor-recovery"
        target_availability_zone: "remote-az"

      # Protection domain settings
      protection_domain: "couchestor-pd"
      remote_site: "dr-site"

      # RPO/RTO targets
      rpo_minutes: 15
      rto_minutes: 60

      # Volume protection
      volume_protection:
        hot_tier:
          replication: sync
          snapshots_per_day: 24
        warm_tier:
          replication: async
          snapshots_per_day: 12
        cold_tier:
          replication: async
          snapshots_per_day: 4
```

### 2. Create Recovery Plan

```python
# Via Prism Central API
recovery_plan = {
    "spec": {
        "name": "couchestor-recovery",
        "description": "Recovery plan for CoucheStor volumes",
        "resources": {
            "parameters": {
                "network_mappings": [
                    {
                        "availability_zone_url": "az://primary",
                        "recovery_network": "VM-Network-DR"
                    }
                ],
                "category_mappings": [
                    {
                        "source_category": {"key": "CoucheStor", "value": "Hot"},
                        "target_category": {"key": "CoucheStor", "value": "Hot"}
                    }
                ]
            },
            "stage_list": [
                {
                    "stage_uuid": "stage-1",
                    "delay_time_secs": 0,
                    "stage_work": {
                        "recover_entities": {
                            "entity_info_list": [
                                {
                                    "categories": {"CoucheStor": ["Hot", "Warm", "Cold"]}
                                }
                            ]
                        }
                    }
                }
            ]
        }
    }
}
```

---

## Verification and Testing

### 1. Verify Installation

```bash
# Check all CoucheStor pods
kubectl -n couchestor-system get pods -o wide

# Check Nutanix CSI driver
kubectl -n ntnx-system get pods

# Verify storage classes
kubectl get sc | grep couchestor

# Check Nutanix connectivity
kubectl -n couchestor-system exec -it deploy/couchestor-controller -- \
  curl -k https://prism-central.example.com:9440/api/nutanix/v3/clusters/list \
  -H "Content-Type: application/json" \
  -u admin:password \
  -d '{}'
```

### 2. Create Test Volume

```yaml
# test-volume-nutanix.yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: test-nutanix-pvc
  namespace: default
  annotations:
    couchestor.io/policy: nutanix-tiered-policy
spec:
  accessModes:
    - ReadWriteOnce
  storageClassName: couchestor-hot
  resources:
    requests:
      storage: 50Gi
---
apiVersion: v1
kind: Pod
metadata:
  name: test-nutanix-pod
  namespace: default
spec:
  containers:
    - name: test
      image: ubuntu:22.04
      command: ["sleep", "infinity"]
      volumeMounts:
        - name: data
          mountPath: /data
  volumes:
    - name: data
      persistentVolumeClaim:
        claimName: test-nutanix-pvc
```

```bash
kubectl apply -f test-volume-nutanix.yaml

# Wait for pod
kubectl wait --for=condition=Ready pod/test-nutanix-pod --timeout=300s

# Verify in Prism
# Check that volume has category CoucheStor=Hot
```

### 3. Test Migration

```bash
# Force tier migration
kubectl annotate pvc test-nutanix-pvc couchestor.io/force-tier=warm

# Monitor migration
kubectl get events -n default --field-selector involvedObject.name=test-nutanix-pvc -w

# Verify in Prism that category changed to CoucheStor=Warm
# Verify volume moved to couchestor-warm-tier container
```

---

## Troubleshooting

### Common Issues

#### 1. CSI Driver Issues

```bash
# Check CSI driver logs
kubectl -n ntnx-system logs -l app=nutanix-csi-node -c nutanix-csi-node
kubectl -n ntnx-system logs -l app=nutanix-csi-controller

# Verify iSCSI connection
kubectl exec -it ds/couchestor-agent -- iscsiadm -m session

# Check secret
kubectl -n ntnx-system get secret ntnx-secret -o yaml
```

#### 2. Prism Connectivity

```bash
# Test API access
curl -k -X POST \
  https://prism-central.example.com:9440/api/nutanix/v3/clusters/list \
  -H "Content-Type: application/json" \
  -u admin:password \
  -d '{}'

# Check certificate issues
openssl s_client -connect prism-central.example.com:9440 -showcerts
```

#### 3. Volume Stuck in Pending

```bash
# Check PVC events
kubectl describe pvc <pvc-name>

# Check CSI provisioner logs
kubectl -n ntnx-system logs -l app=nutanix-csi-controller -c csi-provisioner

# Verify storage container has space
ncli container list name=couchestor-hot-tier
```

#### 4. Migration Failures

```bash
# Check controller logs
kubectl -n couchestor-system logs -l component=controller | grep -i migration

# Verify category exists
curl -k -X GET \
  "https://prism-central.example.com:9440/api/nutanix/v3/categories/CoucheStor" \
  -H "Content-Type: application/json" \
  -u admin:password

# Check volume can be moved between containers
ncli volume-group list
```

### Debug Mode

```bash
# Enable debug logging
kubectl -n couchestor-system set env deploy/couchestor-controller COUCHESTOR_LOG_LEVEL=debug

# Collect all logs
kubectl -n couchestor-system logs -l app=couchestor --all-containers --timestamps > debug.log
kubectl -n ntnx-system logs -l app=nutanix-csi-controller --all-containers >> debug.log
```

---

## Appendix

### A. Calm Blueprint for Automated Deployment

```yaml
# calm-blueprint.yaml (conceptual)
name: CoucheStor-Deployment
description: Automated CoucheStor deployment on NKE
services:
  - name: CoucheStor
    type: Kubernetes
    substrate:
      kind: KUBERNETES_POD
    actions:
      - name: Deploy
        type: PROVISION
        runbook:
          - type: EXEC
            script: |
              kubectl apply -f https://raw.githubusercontent.com/abiolaogu/couchestor/main/deploy/nutanix/all-in-one.yaml
```

### B. ncli Commands Reference

```bash
# List storage containers
ncli container list

# List volume groups
ncli volume-group list

# Get cluster info
ncli cluster info

# Check storage pool usage
ncli storagepool list

# List VMs with storage info
ncli vm list include-storage-info=true
```

### C. Cleanup

```bash
# Delete test resources
kubectl delete -f test-volume-nutanix.yaml

# Uninstall CoucheStor
kubectl delete -f couchestor-agent-nutanix.yaml
kubectl delete -f couchestor-deployment-nutanix.yaml
kubectl delete -f couchestor-config-nutanix.yaml
kubectl delete -f storage-classes-nutanix.yaml
kubectl delete -f deploy/crds/
kubectl delete namespace couchestor-system

# Uninstall Nutanix CSI
helm uninstall nutanix-csi -n ntnx-system
kubectl delete namespace ntnx-system

# Remove storage containers (Prism)
ncli container remove name=couchestor-hot-tier
ncli container remove name=couchestor-warm-tier
ncli container remove name=couchestor-cold-tier
```

---

## Support

- **Documentation**: https://github.com/abiolaogu/couchestor/docs
- **Issues**: https://github.com/abiolaogu/couchestor/issues
- **Nutanix Integration**: https://github.com/abiolaogu/couchestor/docs/nutanix
- **Nutanix Portal**: https://portal.nutanix.com

---

*Last Updated: February 2026*
*CoucheStor Version: 0.1.0*
*Nutanix AOS Version: 6.5+*
*NKE Version: 2.8+*
