# CoucheStor Installation Guide for Apache CloudStack

This guide provides comprehensive instructions for deploying CoucheStor on Apache CloudStack with Kubernetes (CloudMonkey or CloudStack Kubernetes Service).

## Table of Contents

1. [Prerequisites](#prerequisites)
2. [Architecture Overview](#architecture-overview)
3. [CloudStack Environment Preparation](#cloudstack-environment-preparation)
4. [Kubernetes Cluster Deployment](#kubernetes-cluster-deployment)
5. [Storage Configuration](#storage-configuration)
6. [Network Configuration](#network-configuration)
7. [Deploying CoucheStor](#deploying-couchestor)
8. [CloudStack CSI Integration](#cloudstack-csi-integration)
9. [Primary Storage Integration](#primary-storage-integration)
10. [Secondary Storage Integration](#secondary-storage-integration)
11. [Monitoring Integration](#monitoring-integration)
12. [Multi-Zone Deployment](#multi-zone-deployment)
13. [Verification and Testing](#verification-and-testing)
14. [Troubleshooting](#troubleshooting)
15. [Appendix](#appendix)

---

## Prerequisites

### CloudStack Requirements

| Component | Version | Required |
|-----------|---------|----------|
| CloudStack | 4.18+ | Yes |
| CloudStack Kubernetes Service | 1.1+ | Optional |
| Primary Storage | NFS, Ceph, or Local | Yes |
| Secondary Storage | NFS or Object Store | Yes |
| CloudStack CSI Driver | 0.5+ | Yes |

### Hardware Requirements

| Resource | Minimum | Recommended |
|----------|---------|-------------|
| Management Server | 4 vCPU, 8 GB RAM | 8 vCPU, 16 GB RAM |
| Computing Host | 8 vCPU, 32 GB RAM | 32+ vCPU, 128 GB+ RAM |
| Primary Storage | 500 GB | 10 TB+ (SSD/NVMe) |
| Secondary Storage | 1 TB | 5 TB+ |
| Network | 1 GbE | 10/25 GbE |

### Supported Hypervisors

- KVM (recommended)
- VMware vSphere
- Citrix Hypervisor (XenServer)

### Network Requirements

- Management network for CloudStack
- Guest network(s) for VMs
- Storage network (dedicated recommended)
- Public network for external access

### Tools Required

```bash
# CloudStack CLI
pip install cloudmonkey

# Configure CloudMonkey
cloudmonkey set url http://cloudstack.example.com:8080/client/api
cloudmonkey set apikey <your-api-key>
cloudmonkey set secretkey <your-secret-key>

# Kubernetes tools
curl -LO "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
chmod +x kubectl && sudo mv kubectl /usr/local/bin/

# Helm
curl https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash
```

---

## Architecture Overview

### CoucheStor on CloudStack Topology

```
┌───────────────────────────────────────────────────────────────────────────────────┐
│                           CloudStack Infrastructure                                │
│                                                                                    │
│  ┌───────────────────────────────────────────────────────────────────────────┐    │
│  │                    Management Server                                       │    │
│  │  ┌────────────────┐  ┌────────────────┐  ┌────────────────────────────┐   │    │
│  │  │  API Server    │  │  Usage Server  │  │  CloudStack Kubernetes    │   │    │
│  │  │  (8080/8443)   │  │                │  │  Service (CKS)            │   │    │
│  │  └────────────────┘  └────────────────┘  └────────────────────────────┘   │    │
│  └───────────────────────────────────────────────────────────────────────────┘    │
│                                     │                                              │
│  ┌───────────────────────────────────▼───────────────────────────────────────┐    │
│  │                         Zone: zone-1                                       │    │
│  │  ┌─────────────────────────────────────────────────────────────────────┐  │    │
│  │  │                    Pod: pod-1                                        │  │    │
│  │  │  ┌────────────┐  ┌────────────┐  ┌────────────┐                     │  │    │
│  │  │  │  Host 1    │  │  Host 2    │  │  Host 3    │  (KVM Hypervisors)  │  │    │
│  │  │  │ ┌────────┐ │  │ ┌────────┐ │  │ ┌────────┐ │                     │  │    │
│  │  │  │ │K8s Node│ │  │ │K8s Node│ │  │ │K8s Node│ │                     │  │    │
│  │  │  │ │┌──────┐│ │  │ │┌──────┐│ │  │ │┌──────┐│ │                     │  │    │
│  │  │  │ ││Couche││ │  │ ││Couche││ │  │ ││Couche││ │                     │  │    │
│  │  │  │ ││Stor  ││ │  │ ││Stor  ││ │  │ ││Stor  ││ │                     │  │    │
│  │  │  │ │└──────┘│ │  │ │└──────┘│ │  │ │└──────┘│ │                     │  │    │
│  │  │  │ └────────┘ │  │ └────────┘ │  │ └────────┘ │                     │  │    │
│  │  │  └────────────┘  └────────────┘  └────────────┘                     │  │    │
│  │  └─────────────────────────────────────────────────────────────────────┘  │    │
│  │                              │                                              │    │
│  │  ┌───────────────────────────▼─────────────────────────────────────────┐   │    │
│  │  │                    Primary Storage                                   │   │    │
│  │  │  ┌────────────────┐  ┌────────────────┐  ┌────────────────────┐     │   │    │
│  │  │  │  NVMe Pool     │  │   SSD Pool     │  │    HDD Pool        │     │   │    │
│  │  │  │  (Hot Tier)    │  │  (Warm Tier)   │  │   (Cold Tier)      │     │   │    │
│  │  │  └────────────────┘  └────────────────┘  └────────────────────┘     │   │    │
│  │  └─────────────────────────────────────────────────────────────────────┘   │    │
│  │                                                                             │    │
│  │  ┌─────────────────────────────────────────────────────────────────────┐   │    │
│  │  │                    Secondary Storage (NFS/Object)                    │   │    │
│  │  │  ┌────────────────────────────────────────────────────────────────┐ │   │    │
│  │  │  │  Templates  │  Snapshots  │  ISOs  │  CoucheStor Cold Objects  │ │   │    │
│  │  │  └────────────────────────────────────────────────────────────────┘ │   │    │
│  │  └─────────────────────────────────────────────────────────────────────┘   │    │
│  └────────────────────────────────────────────────────────────────────────────┘    │
└────────────────────────────────────────────────────────────────────────────────────┘
```

### Integration Points

1. **CloudStack CSI Driver**: Dynamic volume provisioning
2. **Primary Storage**: Tiered storage pools (NVMe/SSD/HDD)
3. **Secondary Storage**: Cold tier object storage
4. **CloudStack API**: Automation and lifecycle management

---

## CloudStack Environment Preparation

### 1. Configure CloudMonkey

```bash
# Set up CloudMonkey profile
cloudmonkey set profile couchestor
cloudmonkey set url http://cloudstack.example.com:8080/client/api
cloudmonkey set apikey <api-key>
cloudmonkey set secretkey <secret-key>
cloudmonkey set output json

# Verify connection
cloudmonkey list zones
```

### 2. Create Zone and Pod (if not exists)

```bash
# Create zone
cloudmonkey create zone \
  name=couchestor-zone \
  dns1=8.8.8.8 \
  internaldns1=192.168.1.1 \
  networktype=Advanced

# Create pod
cloudmonkey create pod \
  name=couchestor-pod \
  zoneid=<zone-id> \
  startip=192.168.10.100 \
  endip=192.168.10.200 \
  gateway=192.168.10.1 \
  netmask=255.255.255.0
```

### 3. Create Storage Pools for Tiering

```bash
# Create Hot Tier Pool (NVMe/SSD)
cloudmonkey create storagepool \
  name=couchestor-hot-pool \
  zoneid=<zone-id> \
  podid=<pod-id> \
  clusterid=<cluster-id> \
  url="nfs://storage-server/hot-tier" \
  scope=cluster \
  tags=hot,nvme,ssd

# Create Warm Tier Pool (SSD/SAS)
cloudmonkey create storagepool \
  name=couchestor-warm-pool \
  zoneid=<zone-id> \
  podid=<pod-id> \
  clusterid=<cluster-id> \
  url="nfs://storage-server/warm-tier" \
  scope=cluster \
  tags=warm,ssd

# Create Cold Tier Pool (HDD)
cloudmonkey create storagepool \
  name=couchestor-cold-pool \
  zoneid=<zone-id> \
  podid=<pod-id> \
  clusterid=<cluster-id> \
  url="nfs://storage-server/cold-tier" \
  scope=cluster \
  tags=cold,hdd

# Verify pools
cloudmonkey list storagepools
```

### 4. Create Disk Offerings for Tiers

```bash
# Hot Tier Disk Offering
cloudmonkey create diskoffering \
  name="CoucheStor-Hot" \
  displaytext="CoucheStor Hot Tier (NVMe/SSD)" \
  storagetype=shared \
  tags=hot \
  customized=true \
  provisioningtype=thin

# Warm Tier Disk Offering
cloudmonkey create diskoffering \
  name="CoucheStor-Warm" \
  displaytext="CoucheStor Warm Tier (SSD)" \
  storagetype=shared \
  tags=warm \
  customized=true \
  provisioningtype=thin

# Cold Tier Disk Offering
cloudmonkey create diskoffering \
  name="CoucheStor-Cold" \
  displaytext="CoucheStor Cold Tier (HDD)" \
  storagetype=shared \
  tags=cold \
  customized=true \
  provisioningtype=thin

# Verify disk offerings
cloudmonkey list diskofferings | jq '.diskoffering[] | select(.name | startswith("CoucheStor"))'
```

### 5. Create Service Offerings for K8s VMs

```bash
# K8s Master Service Offering
cloudmonkey create serviceoffering \
  name="CoucheStor-K8s-Master" \
  displaytext="K8s Master - 4 vCPU, 8 GB RAM" \
  cpunumber=4 \
  cpuspeed=2000 \
  memory=8192

# K8s Worker Service Offering
cloudmonkey create serviceoffering \
  name="CoucheStor-K8s-Worker" \
  displaytext="K8s Worker - 8 vCPU, 32 GB RAM" \
  cpunumber=8 \
  cpuspeed=2000 \
  memory=32768 \
  tags=storage
```

---

## Kubernetes Cluster Deployment

### Option 1: CloudStack Kubernetes Service (CKS) - Recommended

#### 1. Register Kubernetes ISO

```bash
# Register CoreOS or Ubuntu K8s ISO
cloudmonkey register iso \
  name="Ubuntu-22.04-K8s" \
  displaytext="Ubuntu 22.04 with Kubernetes" \
  url="http://iso-server/ubuntu-22.04-k8s.iso" \
  zoneid=<zone-id> \
  ostypeid=<ubuntu-64-id> \
  isextractable=true \
  isfeatured=true \
  ispublic=true
```

#### 2. Create Kubernetes Cluster via CKS

```bash
# List available Kubernetes versions
cloudmonkey list kubernetessupportedversions

# Create Kubernetes cluster
cloudmonkey create kubernetescluster \
  name=couchestor-k8s \
  description="CoucheStor Kubernetes Cluster" \
  zoneid=<zone-id> \
  kubernetessversionid=<k8s-version-id> \
  serviceofferingid=<master-service-offering-id> \
  size=3 \
  masternodes=3 \
  networkid=<network-id> \
  keypair=<keypair-name>

# Wait for cluster to be ready
watch cloudmonkey list kubernetesclusters name=couchestor-k8s

# Get kubeconfig
cloudmonkey get kubernetesclusterconfig id=<cluster-id>
```

### Option 2: Manual Kubernetes Deployment

#### 1. Create VMs

```bash
# Create master VMs
for i in 1 2 3; do
  cloudmonkey deploy virtualmachine \
    name=k8s-master-$i \
    zoneid=<zone-id> \
    templateid=<ubuntu-template-id> \
    serviceofferingid=<master-offering-id> \
    networkids=<network-id> \
    keypair=<keypair-name>
done

# Create worker VMs
for i in 1 2 3; do
  cloudmonkey deploy virtualmachine \
    name=k8s-worker-$i \
    zoneid=<zone-id> \
    templateid=<ubuntu-template-id> \
    serviceofferingid=<worker-offering-id> \
    networkids=<network-id> \
    keypair=<keypair-name> \
    affinitygroupids=<anti-affinity-group-id>
done
```

#### 2. Initialize Kubernetes Cluster

```bash
# On first master node
kubeadm init \
  --control-plane-endpoint "k8s-api.example.com:6443" \
  --pod-network-cidr=10.244.0.0/16 \
  --upload-certs

# Install CNI (Calico or Flannel)
kubectl apply -f https://raw.githubusercontent.com/flannel-io/flannel/master/Documentation/kube-flannel.yml

# Join other masters and workers
kubeadm join k8s-api.example.com:6443 --token <token> --discovery-token-ca-cert-hash <hash>
```

---

## Storage Configuration

### 1. Install CloudStack CSI Driver

```bash
# Clone CSI driver repository
git clone https://github.com/apache/cloudstack-kubernetes-provider.git
cd cloudstack-kubernetes-provider

# Create namespace
kubectl create namespace cloudstack-csi

# Create CloudStack configuration secret
cat > cloudstack.ini << EOF
[Global]
api-url = http://cloudstack.example.com:8080/client/api
api-key = <your-api-key>
secret-key = <your-secret-key>
ssl-no-verify = true
EOF

kubectl create secret generic cloudstack-secret \
  --namespace cloudstack-csi \
  --from-file=cloudstack.ini

# Deploy CSI driver
kubectl apply -f deploy/csi/
```

### 2. Create Storage Classes

```yaml
# storage-classes-cloudstack.yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-hot
  annotations:
    storageclass.kubernetes.io/is-default-class: "false"
provisioner: csi.cloudstack.apache.org
parameters:
  diskOfferingId: "<hot-tier-disk-offering-id>"
  zoneId: "<zone-id>"
reclaimPolicy: Delete
allowVolumeExpansion: true
volumeBindingMode: WaitForFirstConsumer
---
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-warm
provisioner: csi.cloudstack.apache.org
parameters:
  diskOfferingId: "<warm-tier-disk-offering-id>"
  zoneId: "<zone-id>"
reclaimPolicy: Delete
allowVolumeExpansion: true
volumeBindingMode: WaitForFirstConsumer
---
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-cold
provisioner: csi.cloudstack.apache.org
parameters:
  diskOfferingId: "<cold-tier-disk-offering-id>"
  zoneId: "<zone-id>"
reclaimPolicy: Delete
allowVolumeExpansion: true
volumeBindingMode: WaitForFirstConsumer
```

```bash
kubectl apply -f storage-classes-cloudstack.yaml
```

### 3. Verify CSI Driver

```bash
# Check CSI pods
kubectl -n cloudstack-csi get pods

# Check CSI driver registration
kubectl get csidrivers

# Test volume provisioning
cat > test-pvc.yaml << EOF
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: test-csi-pvc
spec:
  accessModes:
    - ReadWriteOnce
  storageClassName: couchestor-hot
  resources:
    requests:
      storage: 10Gi
EOF

kubectl apply -f test-pvc.yaml
kubectl get pvc test-csi-pvc
```

---

## Network Configuration

### 1. Configure Networks in CloudStack

```bash
# Create isolated network for K8s
cloudmonkey create network \
  name=couchestor-k8s-network \
  displaytext="CoucheStor Kubernetes Network" \
  zoneid=<zone-id> \
  networkofferingid=<isolated-network-offering-id> \
  gateway=10.1.1.1 \
  netmask=255.255.255.0

# Create storage network (optional, for dedicated storage traffic)
cloudmonkey create network \
  name=couchestor-storage-network \
  displaytext="CoucheStor Storage Network" \
  zoneid=<zone-id> \
  networkofferingid=<shared-network-offering-id> \
  vlan=100 \
  gateway=10.2.1.1 \
  netmask=255.255.255.0
```

### 2. Configure Firewall Rules

```bash
# Allow K8s API access
cloudmonkey create firewallrule \
  ipaddressid=<public-ip-id> \
  protocol=TCP \
  startport=6443 \
  endport=6443 \
  cidrlist=0.0.0.0/0

# Allow NodePort range
cloudmonkey create firewallrule \
  ipaddressid=<public-ip-id> \
  protocol=TCP \
  startport=30000 \
  endport=32767 \
  cidrlist=0.0.0.0/0
```

### 3. Configure Kubernetes Network Policies

```yaml
# network-policies-cloudstack.yaml
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
          port: 8080   # CloudStack API
        - protocol: TCP
          port: 8443   # CloudStack API (SSL)
        - protocol: TCP
          port: 9090   # Prometheus
```

```bash
kubectl apply -f network-policies-cloudstack.yaml
```

---

## Deploying CoucheStor

### 1. Create Namespace and Secrets

```bash
# Create namespace
kubectl create namespace couchestor-system

# Create CloudStack credentials secret
kubectl create secret generic cloudstack-credentials \
  --namespace couchestor-system \
  --from-literal=API_URL=http://cloudstack.example.com:8080/client/api \
  --from-literal=API_KEY=<your-api-key> \
  --from-literal=SECRET_KEY=<your-secret-key> \
  --from-literal=ZONE_ID=<zone-id>
```

### 2. Install CRDs

```bash
git clone https://github.com/abiolaogu/couchestor.git
cd couchestor
kubectl apply -f deploy/crds/
```

### 3. Configure CoucheStor for CloudStack

```yaml
# couchestor-config-cloudstack.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: couchestor-config
  namespace: couchestor-system
data:
  config.yaml: |
    cloud_provider: cloudstack

    cloudstack:
      api_url: "http://cloudstack.example.com:8080/client/api"
      zone_id: "<zone-id>"
      credential_ref:
        secret_name: cloudstack-credentials
        namespace: couchestor-system

      # Storage pool mapping
      storage_pools:
        hot: "couchestor-hot-pool"
        warm: "couchestor-warm-pool"
        cold: "couchestor-cold-pool"

      # Disk offering mapping
      disk_offerings:
        hot: "<hot-disk-offering-id>"
        warm: "<warm-disk-offering-id>"
        cold: "<cold-disk-offering-id>"

    prometheus:
      endpoint: "http://prometheus-server.monitoring.svc:9090"
      query_timeout: 30s

    tiers:
      hot:
        storage_class: "couchestor-hot"
        storage_pool: "couchestor-hot-pool"
        iops_threshold: 100
        min_capacity_gb: 100
      warm:
        storage_class: "couchestor-warm"
        storage_pool: "couchestor-warm-pool"
        iops_threshold: 10
        min_capacity_gb: 500
      cold:
        storage_class: "couchestor-cold"
        storage_pool: "couchestor-cold-pool"
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
      # CloudStack-specific: Use volume migration API
      use_cloudstack_migration: true

    cache:
      l1_size_mb: 1024
      l2_size_mb: 10240
      compression: zstd
      prefetch_enabled: true
```

```bash
kubectl apply -f couchestor-config-cloudstack.yaml
```

### 4. Deploy CoucheStor Controller

```yaml
# couchestor-deployment-cloudstack.yaml
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
            - --cloud-provider=cloudstack
          ports:
            - containerPort: 8080
              name: metrics
            - containerPort: 8081
              name: health
          env:
            - name: CLOUDSTACK_API_URL
              valueFrom:
                secretKeyRef:
                  name: cloudstack-credentials
                  key: API_URL
            - name: CLOUDSTACK_API_KEY
              valueFrom:
                secretKeyRef:
                  name: cloudstack-credentials
                  key: API_KEY
            - name: CLOUDSTACK_SECRET_KEY
              valueFrom:
                secretKeyRef:
                  name: cloudstack-credentials
                  key: SECRET_KEY
            - name: CLOUDSTACK_ZONE_ID
              valueFrom:
                secretKeyRef:
                  name: cloudstack-credentials
                  key: ZONE_ID
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
kubectl apply -f couchestor-deployment-cloudstack.yaml
```

### 5. Deploy CoucheStor Agent

```yaml
# couchestor-agent-cloudstack.yaml
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
            - --cloud-provider=cloudstack
          env:
            - name: NODE_NAME
              valueFrom:
                fieldRef:
                  fieldPath: spec.nodeName
            - name: CLOUDSTACK_API_URL
              valueFrom:
                secretKeyRef:
                  name: cloudstack-credentials
                  key: API_URL
            - name: CLOUDSTACK_API_KEY
              valueFrom:
                secretKeyRef:
                  name: cloudstack-credentials
                  key: API_KEY
            - name: CLOUDSTACK_SECRET_KEY
              valueFrom:
                secretKeyRef:
                  name: cloudstack-credentials
                  key: SECRET_KEY
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
      volumes:
        - name: dev
          hostPath:
            path: /dev
        - name: sys
          hostPath:
            path: /sys
      tolerations:
        - operator: Exists
```

```bash
kubectl apply -f couchestor-agent-cloudstack.yaml
```

### 6. Create RBAC

```yaml
# rbac-cloudstack.yaml
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
kubectl apply -f rbac-cloudstack.yaml
```

---

## CloudStack CSI Integration

### Understanding CloudStack Volume Migrations

CoucheStor uses CloudStack APIs for volume lifecycle management:

1. **migrateVolume**: Move volumes between storage pools
2. **createSnapshot**: Create volume snapshots during migration
3. **resizeVolume**: Expand volumes as needed

### Storage Policy with CloudStack Features

```yaml
# storage-policy-cloudstack.yaml
apiVersion: couchestor.io/v1alpha1
kind: StoragePolicy
metadata:
  name: cloudstack-tiered-policy
  namespace: couchestor-system
spec:
  cloudProvider: cloudstack

  cloudstackConfig:
    zoneId: "<zone-id>"

    # Storage pool mapping
    storagePools:
      hot: "couchestor-hot-pool"
      warm: "couchestor-warm-pool"
      cold: "couchestor-cold-pool"

    # Enable live migration
    liveMigration: true

  tiers:
    - name: hot
      storageClass: couchestor-hot
      storagePoolName: "couchestor-hot-pool"
      iopsThreshold: 100
      retentionDays: 7
    - name: warm
      storageClass: couchestor-warm
      storagePoolName: "couchestor-warm-pool"
      iopsThreshold: 10
      retentionDays: 30
    - name: cold
      storageClass: couchestor-cold
      storagePoolName: "couchestor-cold-pool"
      iopsThreshold: 0
      retentionDays: 365

  migration:
    strategy: cloudstack-migrate  # cloudstack-migrate, snapshot-restore
    schedule: "0 2 * * *"
    cooldownPeriod: 24h
    maxConcurrent: 3

    cloudstackOptions:
      # Use CloudStack live migration when possible
      preferLiveMigration: true
      # Create snapshot before migration for safety
      snapshotBeforeMigration: true
      # Cleanup source after successful migration
      cleanupAfterMigration: true

  erasureCoding:
    enabled: true
    coldTierOnly: true
    dataShards: 4
    parityShards: 2
```

```bash
kubectl apply -f storage-policy-cloudstack.yaml
```

---

## Primary Storage Integration

### Ceph as Primary Storage

```bash
# Add Ceph cluster as primary storage
cloudmonkey add cluster \
  clustername=ceph-cluster \
  clustertype=CloudManaged \
  hypervisor=KVM \
  podid=<pod-id> \
  zoneid=<zone-id>

# Create Ceph storage pools
cloudmonkey create storagepool \
  name=ceph-hot-pool \
  zoneid=<zone-id> \
  podid=<pod-id> \
  clusterid=<ceph-cluster-id> \
  url="rbd://ceph-monitor:6789/hot-pool/couchestor" \
  scope=cluster \
  provider=DefaultPrimary \
  tags=hot,nvme
```

### NFS Primary Storage

```bash
# Create NFS primary storage for each tier
cloudmonkey create storagepool \
  name=nfs-hot-pool \
  zoneid=<zone-id> \
  podid=<pod-id> \
  clusterid=<cluster-id> \
  url="nfs://nfs-server/hot-tier" \
  scope=cluster \
  tags=hot
```

---

## Secondary Storage Integration

### S3-Compatible Secondary Storage

```bash
# Add S3 secondary storage for cold tier objects
cloudmonkey add secondarystorage \
  url="s3://access-key:secret-key@s3.example.com/couchestor-cold-bucket" \
  zoneid=<zone-id> \
  provider=S3

# Or use CloudStack's object store
cloudmonkey add objectstore \
  name=couchestor-objects \
  url="s3://minio.example.com" \
  accesskey=<access-key> \
  secretkey=<secret-key> \
  bucketname=couchestor-cold
```

### Configure CoucheStor for Object Storage Cold Tier

```yaml
# object-storage-config.yaml
apiVersion: v1
kind: Secret
metadata:
  name: object-storage-credentials
  namespace: couchestor-system
type: Opaque
stringData:
  AWS_ACCESS_KEY_ID: "<access-key>"
  AWS_SECRET_ACCESS_KEY: "<secret-key>"
  AWS_ENDPOINT_URL: "https://s3.example.com"
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: object-storage-config
  namespace: couchestor-system
data:
  config.yaml: |
    object_storage:
      enabled: true
      endpoint: "https://s3.example.com"
      bucket: "couchestor-cold-tier"
      region: "us-east-1"

      # Use for cold tier objects
      use_for_cold_tier: true

      # Lifecycle policies
      lifecycle:
        expire_days: 365
        transition_to_glacier_days: 90
```

---

## Monitoring Integration

### 1. Deploy Prometheus Stack

```bash
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm install prometheus prometheus-community/kube-prometheus-stack \
  --namespace monitoring \
  --create-namespace
```

### 2. Configure CloudStack Metrics Exporter

```yaml
# cloudstack-exporter.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: cloudstack-exporter
  namespace: monitoring
spec:
  replicas: 1
  selector:
    matchLabels:
      app: cloudstack-exporter
  template:
    metadata:
      labels:
        app: cloudstack-exporter
    spec:
      containers:
        - name: exporter
          image: ghcr.io/abiolaogu/cloudstack-exporter:latest
          ports:
            - containerPort: 9200
          env:
            - name: CLOUDSTACK_API_URL
              valueFrom:
                secretKeyRef:
                  name: cloudstack-credentials
                  key: API_URL
            - name: CLOUDSTACK_API_KEY
              valueFrom:
                secretKeyRef:
                  name: cloudstack-credentials
                  key: API_KEY
            - name: CLOUDSTACK_SECRET_KEY
              valueFrom:
                secretKeyRef:
                  name: cloudstack-credentials
                  key: SECRET_KEY
---
apiVersion: v1
kind: Service
metadata:
  name: cloudstack-exporter
  namespace: monitoring
spec:
  selector:
    app: cloudstack-exporter
  ports:
    - port: 9200
```

### 3. Create ServiceMonitors

```yaml
# servicemonitors.yaml
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
---
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: cloudstack-exporter
  namespace: monitoring
spec:
  selector:
    matchLabels:
      app: cloudstack-exporter
  endpoints:
    - port: http
      interval: 60s
```

### 4. Alerting Rules

```yaml
# alerting-rules-cloudstack.yaml
apiVersion: monitoring.coreos.com/v1
kind: PrometheusRule
metadata:
  name: couchestor-alerts
  namespace: monitoring
spec:
  groups:
    - name: couchestor-cloudstack
      rules:
        - alert: CouchestorCloudStackAPIDown
          expr: cloudstack_api_healthy == 0
          for: 5m
          labels:
            severity: critical
          annotations:
            summary: "CloudStack API is unreachable"

        - alert: StoragePoolCapacityLow
          expr: cloudstack_storage_pool_free_bytes / cloudstack_storage_pool_total_bytes < 0.1
          for: 30m
          labels:
            severity: warning
          annotations:
            summary: "Storage pool {{ $labels.pool }} low on space"

        - alert: CouchestorMigrationFailed
          expr: increase(couchestor_migrations_failed_total[1h]) > 3
          for: 10m
          labels:
            severity: warning
          annotations:
            summary: "Multiple CoucheStor migrations failing"
```

---

## Multi-Zone Deployment

### 1. Configure Multi-Zone Support

```yaml
# multi-zone-config.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: couchestor-multizone-config
  namespace: couchestor-system
data:
  zones.yaml: |
    zones:
      - id: "<zone-1-id>"
        name: "zone-1"
        primary: true
        storage_pools:
          hot: "zone1-hot-pool"
          warm: "zone1-warm-pool"
          cold: "zone1-cold-pool"

      - id: "<zone-2-id>"
        name: "zone-2"
        primary: false
        storage_pools:
          hot: "zone2-hot-pool"
          warm: "zone2-warm-pool"
          cold: "zone2-cold-pool"

    replication:
      # Replicate cold tier across zones
      cold_tier_cross_zone: true
      # Use secondary storage for cross-zone replication
      use_secondary_storage: true
```

### 2. Cross-Zone Storage Policy

```yaml
# cross-zone-policy.yaml
apiVersion: couchestor.io/v1alpha1
kind: StoragePolicy
metadata:
  name: cross-zone-policy
spec:
  multiZone:
    enabled: true
    primaryZone: "<zone-1-id>"
    secondaryZones:
      - "<zone-2-id>"

  tiers:
    - name: hot
      zones:
        - "<zone-1-id>"
    - name: warm
      zones:
        - "<zone-1-id>"
        - "<zone-2-id>"
    - name: cold
      zones:
        - "<zone-2-id>"

  replication:
    mode: async
    replicateColdTier: true
```

---

## Verification and Testing

### 1. Verify Installation

```bash
# Check CoucheStor pods
kubectl -n couchestor-system get pods -o wide

# Check CSI driver
kubectl -n cloudstack-csi get pods

# Verify storage classes
kubectl get sc | grep couchestor

# Test CloudStack connectivity
cloudmonkey list zones
cloudmonkey list storagepools
```

### 2. Create Test Workload

```yaml
# test-workload-cloudstack.yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: test-cloudstack-pvc
  namespace: default
  annotations:
    couchestor.io/policy: cloudstack-tiered-policy
spec:
  accessModes:
    - ReadWriteOnce
  storageClassName: couchestor-hot
  resources:
    requests:
      storage: 20Gi
---
apiVersion: v1
kind: Pod
metadata:
  name: test-cloudstack-pod
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
        claimName: test-cloudstack-pvc
```

```bash
kubectl apply -f test-workload-cloudstack.yaml

# Wait for pod
kubectl wait --for=condition=Ready pod/test-cloudstack-pod --timeout=300s

# Verify in CloudStack
cloudmonkey list volumes | jq '.volume[] | select(.name | contains("pvc"))'

# Generate I/O
kubectl exec -it test-cloudstack-pod -- dd if=/dev/urandom of=/data/testfile bs=1M count=100
```

### 3. Test Migration

```bash
# Force migration to warm tier
kubectl annotate pvc test-cloudstack-pvc couchestor.io/force-tier=warm

# Monitor migration
kubectl get events -n default --field-selector involvedObject.name=test-cloudstack-pvc -w

# Verify in CloudStack that volume moved to warm pool
cloudmonkey list volumes name=<volume-name> | jq '.volume[0].storage'
```

---

## Troubleshooting

### Common Issues

#### 1. CSI Driver Issues

```bash
# Check CSI driver pods
kubectl -n cloudstack-csi get pods

# Check CSI driver logs
kubectl -n cloudstack-csi logs -l app=cloudstack-csi-controller
kubectl -n cloudstack-csi logs -l app=cloudstack-csi-node

# Verify CloudStack configuration
kubectl -n cloudstack-csi get secret cloudstack-secret -o jsonpath='{.data.cloudstack\.ini}' | base64 -d
```

#### 2. CloudStack API Issues

```bash
# Test API connectivity
cloudmonkey list zones

# Check API credentials
curl "http://cloudstack.example.com:8080/client/api?command=listZones&response=json&apikey=<api-key>&signature=<signature>"

# Verify user permissions
cloudmonkey list accounts name=<account-name>
```

#### 3. Volume Stuck in Creating

```bash
# Check PVC events
kubectl describe pvc <pvc-name>

# Check CloudStack volume status
cloudmonkey list volumes state=Allocated

# Check storage pool capacity
cloudmonkey list storagepools name=<pool-name> | jq '.storagepool[0] | {capacitybytes, usedbytes}'
```

#### 4. Migration Failures

```bash
# Check controller logs
kubectl -n couchestor-system logs -l component=controller | grep -i migration

# Check CloudStack async job status
cloudmonkey query asyncjobresult jobid=<job-id>

# Verify storage pool connectivity
cloudmonkey list hosts type=Routing state=Up
```

### Debug Mode

```bash
# Enable debug logging
kubectl -n couchestor-system set env deploy/couchestor-controller COUCHESTOR_LOG_LEVEL=debug

# Collect logs
kubectl -n couchestor-system logs -l app=couchestor --all-containers --timestamps > debug.log
kubectl -n cloudstack-csi logs -l app=cloudstack-csi-controller >> debug.log
```

---

## Appendix

### A. CloudMonkey Commands Reference

```bash
# Zone management
cloudmonkey list zones
cloudmonkey create zone name=<name> dns1=<dns> internaldns1=<internal-dns> networktype=<type>

# Storage management
cloudmonkey list storagepools
cloudmonkey create storagepool name=<name> zoneid=<id> url=<url> scope=<scope>
cloudmonkey list diskofferings
cloudmonkey create diskoffering name=<name> storagetype=<type> tags=<tags>

# Volume management
cloudmonkey list volumes
cloudmonkey migrate volume volumeid=<id> storageid=<pool-id>
cloudmonkey create snapshot volumeid=<id>

# VM management
cloudmonkey deploy virtualmachine
cloudmonkey list virtualmachines
```

### B. Terraform Provider (Alternative)

```hcl
# main.tf
terraform {
  required_providers {
    cloudstack = {
      source  = "cloudstack/cloudstack"
      version = "~> 0.4"
    }
  }
}

provider "cloudstack" {
  api_url    = "http://cloudstack.example.com:8080/client/api"
  api_key    = var.cloudstack_api_key
  secret_key = var.cloudstack_secret_key
}

resource "cloudstack_disk" "hot_tier" {
  name              = "couchestor-hot-disk"
  disk_offering_id  = data.cloudstack_disk_offering.hot.id
  zone_id           = data.cloudstack_zone.main.id
  size              = 100
}
```

### C. Cleanup

```bash
# Delete test resources
kubectl delete -f test-workload-cloudstack.yaml

# Uninstall CoucheStor
kubectl delete -f couchestor-agent-cloudstack.yaml
kubectl delete -f couchestor-deployment-cloudstack.yaml
kubectl delete -f couchestor-config-cloudstack.yaml
kubectl delete -f storage-classes-cloudstack.yaml
kubectl delete -f deploy/crds/
kubectl delete namespace couchestor-system

# Uninstall CSI driver
kubectl delete -f cloudstack-kubernetes-provider/deploy/csi/
kubectl delete namespace cloudstack-csi

# Remove CloudStack resources
cloudmonkey delete storagepool id=<hot-pool-id>
cloudmonkey delete storagepool id=<warm-pool-id>
cloudmonkey delete storagepool id=<cold-pool-id>
cloudmonkey delete diskoffering id=<hot-offering-id>
cloudmonkey delete diskoffering id=<warm-offering-id>
cloudmonkey delete diskoffering id=<cold-offering-id>
```

---

## Support

- **Documentation**: https://github.com/abiolaogu/couchestor/docs
- **Issues**: https://github.com/abiolaogu/couchestor/issues
- **CloudStack Integration**: https://github.com/abiolaogu/couchestor/docs/cloudstack
- **Apache CloudStack**: https://cloudstack.apache.org/

---

*Last Updated: February 2026*
*CoucheStor Version: 0.1.0*
*Apache CloudStack Version: 4.18+*
