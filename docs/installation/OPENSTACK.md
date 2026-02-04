# CoucheStor Installation Guide for OpenStack

This guide provides comprehensive instructions for deploying CoucheStor on OpenStack cloud infrastructure with Kubernetes (via Magnum or manual deployment).

## Table of Contents

1. [Prerequisites](#prerequisites)
2. [Architecture Overview](#architecture-overview)
3. [OpenStack Environment Preparation](#openstack-environment-preparation)
4. [Kubernetes Cluster Deployment](#kubernetes-cluster-deployment)
5. [Storage Backend Configuration](#storage-backend-configuration)
6. [Network Configuration](#network-configuration)
7. [Deploying CoucheStor](#deploying-couchestor)
8. [Cinder Integration](#cinder-integration)
9. [Manila Integration (Optional)](#manila-integration-optional)
10. [Monitoring with Ceilometer](#monitoring-with-ceilometer)
11. [Multi-Region Deployment](#multi-region-deployment)
12. [Verification and Testing](#verification-and-testing)
13. [Troubleshooting](#troubleshooting)
14. [Appendix](#appendix)

---

## Prerequisites

### OpenStack Requirements

| Component | Version | Required |
|-----------|---------|----------|
| OpenStack Release | Zed or later | Yes |
| Keystone | v3 API | Yes |
| Nova | - | Yes |
| Cinder | v3 API | Yes |
| Neutron | - | Yes |
| Glance | - | Yes |
| Heat (optional) | - | Recommended |
| Magnum (optional) | - | Recommended |
| Manila (optional) | - | For shared storage |
| Ceilometer (optional) | - | For metrics |

### Compute Requirements

| Resource | Minimum | Recommended |
|----------|---------|-------------|
| vCPUs per node | 4 | 16+ |
| RAM per node | 16 GB | 64 GB+ |
| System disk | 80 GB | 200 GB SSD |
| Data volumes | 500 GB | 2 TB+ per tier |
| Nodes | 3 | 5+ |

### Network Requirements

- Private network for Kubernetes cluster
- Floating IPs for external access
- Security groups configured for K8s traffic
- Load balancer (Octavia) recommended

### Tools Required

```bash
# OpenStack CLI
pip install python-openstackclient python-magnumclient python-cinderclient

# Kubernetes tools
curl -LO "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
chmod +x kubectl && sudo mv kubectl /usr/local/bin/

# Helm
curl https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash
```

---

## Architecture Overview

### CoucheStor on OpenStack Topology

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           OpenStack Cloud                                    │
│                                                                             │
│  ┌──────────────────────────────────────────────────────────────────────┐  │
│  │                    Kubernetes Cluster (Magnum)                        │  │
│  │                                                                       │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                   │  │
│  │  │  Master 1   │  │  Master 2   │  │  Master 3   │  Control Plane    │  │
│  │  └─────────────┘  └─────────────┘  └─────────────┘                   │  │
│  │                                                                       │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                   │  │
│  │  │  Worker 1   │  │  Worker 2   │  │  Worker 3   │                   │  │
│  │  │┌───────────┐│  │┌───────────┐│  │┌───────────┐│                   │  │
│  │  ││CoucheStor ││  ││CoucheStor ││  ││CoucheStor ││                   │  │
│  │  ││  Agent    ││  ││  Agent    ││  ││  Agent    ││                   │  │
│  │  │└───────────┘│  │└───────────┘│  │└───────────┘│                   │  │
│  │  │┌───────────┐│  │┌───────────┐│  │┌───────────┐│                   │  │
│  │  ││ Cinder    ││  ││ Cinder    ││  ││ Cinder    ││                   │  │
│  │  ││ Volumes   ││  ││ Volumes   ││  ││ Volumes   ││                   │  │
│  │  │└───────────┘│  │└───────────┘│  │└───────────┘│                   │  │
│  │  └─────────────┘  └─────────────┘  └─────────────┘                   │  │
│  │                          │                                            │  │
│  │              ┌───────────▼───────────┐                               │  │
│  │              │  CoucheStor Controller │                               │  │
│  │              └───────────┬───────────┘                               │  │
│  └──────────────────────────│────────────────────────────────────────────┘  │
│                             │                                               │
│  ┌──────────────────────────▼────────────────────────────────────────────┐  │
│  │                     Cinder Storage Backend                            │  │
│  │                                                                       │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                   │  │
│  │  │   SSD Pool  │  │   SAS Pool  │  │  SATA Pool  │                   │  │
│  │  │  (Hot Tier) │  │ (Warm Tier) │  │ (Cold Tier) │                   │  │
│  │  │   NVMe/SSD  │  │    HDD      │  │ Object/HDD  │                   │  │
│  │  └─────────────┘  └─────────────┘  └─────────────┘                   │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
│                                                                             │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │                    Monitoring & Telemetry                             │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                   │  │
│  │  │ Ceilometer  │  │  Gnocchi    │  │  Prometheus │                   │  │
│  │  └─────────────┘  └─────────────┘  └─────────────┘                   │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Integration Points

1. **Cinder CSI**: CoucheStor uses Cinder CSI driver for dynamic volume provisioning
2. **Ceilometer/Gnocchi**: IOPS metrics collection for tier decisions
3. **Neutron**: Network policies and load balancing
4. **Heat**: Infrastructure-as-code deployment (optional)

---

## OpenStack Environment Preparation

### 1. Source OpenStack Credentials

```bash
# Download RC file from Horizon or create manually
cat > ~/openstack-rc.sh << 'EOF'
export OS_AUTH_URL=https://keystone.example.com:5000/v3
export OS_PROJECT_NAME="couchestor-project"
export OS_USER_DOMAIN_NAME="Default"
export OS_PROJECT_DOMAIN_NAME="Default"
export OS_USERNAME="admin"
export OS_PASSWORD="your-password"
export OS_REGION_NAME="RegionOne"
export OS_INTERFACE="public"
export OS_IDENTITY_API_VERSION=3
EOF

source ~/openstack-rc.sh
```

### 2. Create Project and Quotas

```bash
# Create project
openstack project create couchestor-project --domain default

# Set quotas
openstack quota set couchestor-project \
  --instances 20 \
  --cores 200 \
  --ram 512000 \
  --volumes 100 \
  --gigabytes 10000 \
  --floating-ips 10

# Create application credential for CoucheStor
openstack application credential create couchestor-cred \
  --secret "$(openssl rand -hex 32)" \
  --role admin \
  --unrestricted
```

### 3. Create Networks

```bash
# Create private network
openstack network create couchestor-network

# Create subnet
openstack subnet create couchestor-subnet \
  --network couchestor-network \
  --subnet-range 10.10.0.0/24 \
  --gateway 10.10.0.1 \
  --dns-nameserver 8.8.8.8

# Create router and connect to external network
openstack router create couchestor-router
openstack router set couchestor-router --external-gateway external-network
openstack router add subnet couchestor-router couchestor-subnet
```

### 4. Configure Security Groups

```bash
# Create security group for Kubernetes
openstack security group create k8s-security-group

# Allow SSH
openstack security group rule create k8s-security-group \
  --protocol tcp --dst-port 22 --remote-ip 0.0.0.0/0

# Allow Kubernetes API
openstack security group rule create k8s-security-group \
  --protocol tcp --dst-port 6443 --remote-ip 0.0.0.0/0

# Allow NodePort range
openstack security group rule create k8s-security-group \
  --protocol tcp --dst-port 30000:32767 --remote-ip 0.0.0.0/0

# Allow internal cluster communication
openstack security group rule create k8s-security-group \
  --protocol tcp --dst-port 1:65535 --remote-ip 10.10.0.0/24
openstack security group rule create k8s-security-group \
  --protocol udp --dst-port 1:65535 --remote-ip 10.10.0.0/24

# Allow ICMP
openstack security group rule create k8s-security-group \
  --protocol icmp --remote-ip 0.0.0.0/0
```

---

## Kubernetes Cluster Deployment

### Option 1: Using Magnum (Recommended)

#### 1. Create Cluster Template

```bash
# List available images
openstack image list --property os_distro=fedora-coreos

# Create cluster template
openstack coe cluster template create k8s-template \
  --image fedora-coreos-38 \
  --keypair my-keypair \
  --external-network external-network \
  --fixed-network couchestor-network \
  --fixed-subnet couchestor-subnet \
  --network-driver flannel \
  --docker-storage-driver overlay2 \
  --coe kubernetes \
  --master-flavor m1.large \
  --flavor m1.xlarge \
  --volume-driver cinder \
  --labels \
    kube_tag=v1.28.0,\
    cloud_provider_enabled=true,\
    cinder_csi_enabled=true,\
    monitoring_enabled=true,\
    auto_scaling_enabled=true
```

#### 2. Create Cluster

```bash
# Create the cluster
openstack coe cluster create couchestor-cluster \
  --cluster-template k8s-template \
  --master-count 3 \
  --node-count 3 \
  --timeout 90

# Wait for cluster creation
openstack coe cluster show couchestor-cluster

# Get kubeconfig
openstack coe cluster config couchestor-cluster --dir ~/
export KUBECONFIG=~/config

# Verify cluster
kubectl get nodes
```

### Option 2: Manual Kubernetes Deployment

#### 1. Create Instances

```bash
# Create master nodes
for i in 1 2 3; do
  openstack server create \
    --image ubuntu-22.04 \
    --flavor m1.large \
    --network couchestor-network \
    --security-group k8s-security-group \
    --key-name my-keypair \
    --user-data cloud-init-master.yaml \
    k8s-master-$i
done

# Create worker nodes
for i in 1 2 3; do
  openstack server create \
    --image ubuntu-22.04 \
    --flavor m1.xlarge \
    --network couchestor-network \
    --security-group k8s-security-group \
    --key-name my-keypair \
    --user-data cloud-init-worker.yaml \
    k8s-worker-$i
done
```

#### 2. Install Kubernetes with kubeadm

```bash
# On all nodes - cloud-init-common.yaml
cat > cloud-init-common.yaml << 'EOF'
#cloud-config
package_update: true
packages:
  - apt-transport-https
  - ca-certificates
  - curl
  - gnupg

runcmd:
  # Container runtime
  - curl -fsSL https://download.docker.com/linux/ubuntu/gpg | gpg --dearmor -o /etc/apt/keyrings/docker.gpg
  - echo "deb [arch=amd64 signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu jammy stable" > /etc/apt/sources.list.d/docker.list
  - apt-get update && apt-get install -y containerd.io
  - containerd config default > /etc/containerd/config.toml
  - sed -i 's/SystemdCgroup = false/SystemdCgroup = true/' /etc/containerd/config.toml
  - systemctl restart containerd

  # Kubernetes
  - curl -fsSL https://pkgs.k8s.io/core:/stable:/v1.28/deb/Release.key | gpg --dearmor -o /etc/apt/keyrings/kubernetes-apt-keyring.gpg
  - echo 'deb [signed-by=/etc/apt/keyrings/kubernetes-apt-keyring.gpg] https://pkgs.k8s.io/core:/stable:/v1.28/deb/ /' > /etc/apt/sources.list.d/kubernetes.list
  - apt-get update && apt-get install -y kubelet kubeadm kubectl
  - apt-mark hold kubelet kubeadm kubectl

  # Kernel modules
  - |
    cat > /etc/modules-load.d/k8s.conf << EOFMOD
    overlay
    br_netfilter
    EOFMOD
  - modprobe overlay br_netfilter

  # Sysctl
  - |
    cat > /etc/sysctl.d/k8s.conf << EOFSYS
    net.bridge.bridge-nf-call-iptables = 1
    net.bridge.bridge-nf-call-ip6tables = 1
    net.ipv4.ip_forward = 1
    EOFSYS
  - sysctl --system
EOF
```

#### 3. Initialize Cluster with OpenStack Cloud Provider

```bash
# On master node
cat > kubeadm-config.yaml << 'EOF'
apiVersion: kubeadm.k8s.io/v1beta3
kind: ClusterConfiguration
kubernetesVersion: v1.28.0
controlPlaneEndpoint: "10.10.0.10:6443"
networking:
  podSubnet: "10.244.0.0/16"
apiServer:
  extraArgs:
    cloud-provider: external
controllerManager:
  extraArgs:
    cloud-provider: external
---
apiVersion: kubeadm.k8s.io/v1beta3
kind: InitConfiguration
nodeRegistration:
  kubeletExtraArgs:
    cloud-provider: external
EOF

kubeadm init --config kubeadm-config.yaml
```

#### 4. Install OpenStack Cloud Controller Manager

```bash
# Create cloud-config secret
cat > cloud-config << 'EOF'
[Global]
auth-url=https://keystone.example.com:5000/v3
application-credential-id=<credential-id>
application-credential-secret=<credential-secret>
region=RegionOne

[LoadBalancer]
subnet-id=<couchestor-subnet-id>
floating-network-id=<external-network-id>

[BlockStorage]
bs-version=v3
EOF

kubectl create secret -n kube-system generic cloud-config --from-file=cloud.conf=cloud-config

# Install OpenStack Cloud Controller Manager
kubectl apply -f https://raw.githubusercontent.com/kubernetes/cloud-provider-openstack/master/manifests/controller-manager/cloud-controller-manager-roles.yaml
kubectl apply -f https://raw.githubusercontent.com/kubernetes/cloud-provider-openstack/master/manifests/controller-manager/cloud-controller-manager-role-bindings.yaml
kubectl apply -f https://raw.githubusercontent.com/kubernetes/cloud-provider-openstack/master/manifests/controller-manager/openstack-cloud-controller-manager-ds.yaml
```

---

## Storage Backend Configuration

### 1. Configure Cinder Volume Types

```bash
# Create volume types for each tier
openstack volume type create couchestor-hot \
  --property volume_backend_name=nvme-pool \
  --property couchestor_tier=hot

openstack volume type create couchestor-warm \
  --property volume_backend_name=sas-pool \
  --property couchestor_tier=warm

openstack volume type create couchestor-cold \
  --property volume_backend_name=sata-pool \
  --property couchestor_tier=cold

# Verify volume types
openstack volume type list
```

### 2. Install Cinder CSI Driver

```bash
# Create cloud config for CSI
cat > cinder-csi-cloud-config << 'EOF'
[Global]
auth-url=https://keystone.example.com:5000/v3
application-credential-id=<credential-id>
application-credential-secret=<credential-secret>
region=RegionOne
EOF

kubectl create secret -n kube-system generic cinder-csi-cloud-config \
  --from-file=cloud.conf=cinder-csi-cloud-config

# Install Cinder CSI driver via Helm
helm repo add cpo https://kubernetes.github.io/cloud-provider-openstack
helm repo update

helm install cinder-csi cpo/openstack-cinder-csi \
  --namespace kube-system \
  --set secret.enabled=true \
  --set secret.name=cinder-csi-cloud-config
```

### 3. Create Storage Classes

```yaml
# storage-classes.yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-hot
provisioner: cinder.csi.openstack.org
parameters:
  type: couchestor-hot
reclaimPolicy: Delete
volumeBindingMode: WaitForFirstConsumer
allowVolumeExpansion: true
---
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-warm
provisioner: cinder.csi.openstack.org
parameters:
  type: couchestor-warm
reclaimPolicy: Delete
volumeBindingMode: WaitForFirstConsumer
allowVolumeExpansion: true
---
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-cold
provisioner: cinder.csi.openstack.org
parameters:
  type: couchestor-cold
reclaimPolicy: Delete
volumeBindingMode: WaitForFirstConsumer
allowVolumeExpansion: true
```

```bash
kubectl apply -f storage-classes.yaml
```

---

## Network Configuration

### 1. Install Octavia Ingress Controller (Optional)

```bash
# Install Octavia Ingress Controller
helm install octavia-ingress cpo/octavia-ingress-controller \
  --namespace kube-system \
  --set cloudConfig.secret.name=cloud-config
```

### 2. Configure Network Policies

```yaml
# network-policies.yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: couchestor-controller-policy
  namespace: couchestor-system
spec:
  podSelector:
    matchLabels:
      app: couchestor
      component: controller
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
          port: 5000   # Keystone
        - protocol: TCP
          port: 8776   # Cinder
        - protocol: TCP
          port: 9090   # Prometheus
```

```bash
kubectl apply -f network-policies.yaml
```

---

## Deploying CoucheStor

### 1. Create Namespace and Secrets

```bash
# Create namespace
kubectl create namespace couchestor-system

# Create OpenStack credentials secret
kubectl create secret generic openstack-credentials \
  --namespace couchestor-system \
  --from-literal=OS_AUTH_URL=https://keystone.example.com:5000/v3 \
  --from-literal=OS_APPLICATION_CREDENTIAL_ID=<credential-id> \
  --from-literal=OS_APPLICATION_CREDENTIAL_SECRET=<credential-secret> \
  --from-literal=OS_REGION_NAME=RegionOne
```

### 2. Install CoucheStor CRDs

```bash
git clone https://github.com/abiolaogu/couchestor.git
cd couchestor
kubectl apply -f deploy/crds/
```

### 3. Configure CoucheStor for OpenStack

```yaml
# couchestor-config-openstack.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: couchestor-config
  namespace: couchestor-system
data:
  config.yaml: |
    cloud_provider: openstack

    openstack:
      auth_url: "https://keystone.example.com:5000/v3"
      region: "RegionOne"
      credential_ref:
        secret_name: openstack-credentials
        namespace: couchestor-system

    prometheus:
      endpoint: "http://prometheus-server.monitoring.svc:9090"
      query_timeout: 30s
      # Alternative: Use Ceilometer/Gnocchi for metrics
      # ceilometer_endpoint: "http://ceilometer-api.openstack.svc:8777"

    tiers:
      hot:
        storage_class: "couchestor-hot"
        cinder_volume_type: "couchestor-hot"
        iops_threshold: 100
        min_capacity_gb: 100
      warm:
        storage_class: "couchestor-warm"
        cinder_volume_type: "couchestor-warm"
        iops_threshold: 10
        min_capacity_gb: 500
      cold:
        storage_class: "couchestor-cold"
        cinder_volume_type: "couchestor-cold"
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
      # OpenStack-specific: Use Cinder volume transfer
      use_cinder_transfer: true

    cache:
      l1_size_mb: 1024
      l2_size_mb: 10240
      compression: zstd
      prefetch_enabled: true
```

```bash
kubectl apply -f couchestor-config-openstack.yaml
```

### 4. Deploy CoucheStor Controller

```yaml
# couchestor-deployment-openstack.yaml
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
            - --cloud-provider=openstack
          ports:
            - containerPort: 8080
              name: metrics
            - containerPort: 8081
              name: health
          env:
            - name: OS_AUTH_URL
              valueFrom:
                secretKeyRef:
                  name: openstack-credentials
                  key: OS_AUTH_URL
            - name: OS_APPLICATION_CREDENTIAL_ID
              valueFrom:
                secretKeyRef:
                  name: openstack-credentials
                  key: OS_APPLICATION_CREDENTIAL_ID
            - name: OS_APPLICATION_CREDENTIAL_SECRET
              valueFrom:
                secretKeyRef:
                  name: openstack-credentials
                  key: OS_APPLICATION_CREDENTIAL_SECRET
            - name: OS_REGION_NAME
              valueFrom:
                secretKeyRef:
                  name: openstack-credentials
                  key: OS_REGION_NAME
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
kubectl apply -f couchestor-deployment-openstack.yaml
```

### 5. Deploy CoucheStor Agent

```yaml
# couchestor-agent-openstack.yaml
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
            - --cloud-provider=openstack
          env:
            - name: NODE_NAME
              valueFrom:
                fieldRef:
                  fieldPath: spec.nodeName
            - name: OS_AUTH_URL
              valueFrom:
                secretKeyRef:
                  name: openstack-credentials
                  key: OS_AUTH_URL
            - name: OS_APPLICATION_CREDENTIAL_ID
              valueFrom:
                secretKeyRef:
                  name: openstack-credentials
                  key: OS_APPLICATION_CREDENTIAL_ID
            - name: OS_APPLICATION_CREDENTIAL_SECRET
              valueFrom:
                secretKeyRef:
                  name: openstack-credentials
                  key: OS_APPLICATION_CREDENTIAL_SECRET
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
kubectl apply -f couchestor-agent-openstack.yaml
```

### 6. Create RBAC Resources

```yaml
# rbac.yaml
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
    resources: ["volumesnapshots", "volumesnapshotcontents"]
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
kubectl apply -f rbac.yaml
```

---

## Cinder Integration

### Understanding Cinder-Based Migrations

CoucheStor uses Cinder's native features for efficient migrations:

1. **Volume Transfer**: For moving volumes between tiers with different backends
2. **Retype**: For in-place volume type changes when supported
3. **Snapshots**: For data protection during migration

### Configure Migration Strategy

```yaml
# storage-policy-openstack.yaml
apiVersion: couchestor.io/v1alpha1
kind: StoragePolicy
metadata:
  name: openstack-tiered-policy
  namespace: couchestor-system
spec:
  cloudProvider: openstack

  tiers:
    - name: hot
      storageClass: couchestor-hot
      cinderVolumeType: couchestor-hot
      iopsThreshold: 100
      retentionDays: 7
    - name: warm
      storageClass: couchestor-warm
      cinderVolumeType: couchestor-warm
      iopsThreshold: 10
      retentionDays: 30
    - name: cold
      storageClass: couchestor-cold
      cinderVolumeType: couchestor-cold
      iopsThreshold: 0
      retentionDays: 365

  migration:
    strategy: cinder-retype  # Options: cinder-retype, cinder-transfer, snapshot-restore
    schedule: "0 2 * * *"
    cooldownPeriod: 24h
    maxConcurrent: 3

    # Cinder-specific options
    cinderOptions:
      migrationPolicy: on-demand  # on-demand or never
      allowHostCopy: true
      retypeTimeout: 3600  # seconds

  erasureCoding:
    enabled: true
    coldTierOnly: true
    dataShards: 4
    parityShards: 2
```

```bash
kubectl apply -f storage-policy-openstack.yaml
```

---

## Manila Integration (Optional)

For shared filesystem support using OpenStack Manila:

### 1. Install Manila CSI Driver

```bash
helm install manila-csi cpo/openstack-manila-csi \
  --namespace kube-system \
  --set secret.enabled=true \
  --set secret.name=manila-csi-cloud-config
```

### 2. Create Manila Storage Class

```yaml
# manila-storage-class.yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: couchestor-shared
provisioner: manila.csi.openstack.org
parameters:
  type: default_share_type
  shareNetworkID: <share-network-id>
  csi.storage.k8s.io/provisioner-secret-name: manila-csi-cloud-config
  csi.storage.k8s.io/provisioner-secret-namespace: kube-system
reclaimPolicy: Delete
```

---

## Monitoring with Ceilometer

### 1. Configure Ceilometer Metrics Collection

```yaml
# ceilometer-integration.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: couchestor-ceilometer-config
  namespace: couchestor-system
data:
  ceilometer.yaml: |
    endpoint: "http://ceilometer-api.openstack.svc:8777"

    metrics_mapping:
      volume_iops_read: "volume.read.requests.rate"
      volume_iops_write: "volume.write.requests.rate"
      volume_throughput_read: "volume.read.bytes.rate"
      volume_throughput_write: "volume.write.bytes.rate"
      volume_latency: "volume.io.latency"

    aggregation_period: "300"  # 5 minutes

    # Gnocchi configuration (if using)
    gnocchi:
      endpoint: "http://gnocchi-api.openstack.svc:8041"
      archive_policy: "high"
```

### 2. Create Prometheus Exporter for Ceilometer

```yaml
# ceilometer-exporter.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: ceilometer-exporter
  namespace: couchestor-system
spec:
  replicas: 1
  selector:
    matchLabels:
      app: ceilometer-exporter
  template:
    metadata:
      labels:
        app: ceilometer-exporter
    spec:
      containers:
        - name: exporter
          image: ghcr.io/abiolaogu/couchestor-ceilometer-exporter:latest
          ports:
            - containerPort: 9105
          env:
            - name: OS_AUTH_URL
              valueFrom:
                secretKeyRef:
                  name: openstack-credentials
                  key: OS_AUTH_URL
            - name: OS_APPLICATION_CREDENTIAL_ID
              valueFrom:
                secretKeyRef:
                  name: openstack-credentials
                  key: OS_APPLICATION_CREDENTIAL_ID
            - name: OS_APPLICATION_CREDENTIAL_SECRET
              valueFrom:
                secretKeyRef:
                  name: openstack-credentials
                  key: OS_APPLICATION_CREDENTIAL_SECRET
---
apiVersion: v1
kind: Service
metadata:
  name: ceilometer-exporter
  namespace: couchestor-system
spec:
  selector:
    app: ceilometer-exporter
  ports:
    - port: 9105
```

---

## Multi-Region Deployment

### 1. Configure Multi-Region Support

```yaml
# multi-region-config.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: couchestor-multiregion-config
  namespace: couchestor-system
data:
  regions.yaml: |
    primary_region: RegionOne

    regions:
      - name: RegionOne
        endpoint: "https://keystone.region1.example.com:5000/v3"
        availability_zones:
          - nova-az1
          - nova-az2
        storage_backends:
          - name: nvme-pool
            tier: hot
          - name: sas-pool
            tier: warm
          - name: sata-pool
            tier: cold

      - name: RegionTwo
        endpoint: "https://keystone.region2.example.com:5000/v3"
        availability_zones:
          - nova-az1
        storage_backends:
          - name: ssd-pool
            tier: hot
          - name: hdd-pool
            tier: cold

    replication:
      enabled: true
      mode: async
      max_lag_seconds: 300
```

### 2. Cross-Region Storage Policy

```yaml
# cross-region-policy.yaml
apiVersion: couchestor.io/v1alpha1
kind: StoragePolicy
metadata:
  name: cross-region-policy
spec:
  multiRegion:
    enabled: true
    primaryRegion: RegionOne
    replicaRegions:
      - RegionTwo

  tiers:
    - name: hot
      storageClass: couchestor-hot
      regions:
        - RegionOne
    - name: warm
      storageClass: couchestor-warm
      regions:
        - RegionOne
        - RegionTwo
    - name: cold
      storageClass: couchestor-cold
      regions:
        - RegionTwo
```

---

## Verification and Testing

### 1. Verify Installation

```bash
# Check all pods
kubectl -n couchestor-system get pods -o wide

# Check controller logs
kubectl -n couchestor-system logs -l component=controller --tail=50

# Verify OpenStack connectivity
kubectl -n couchestor-system exec -it deploy/couchestor-controller -- \
  openstack volume type list

# Check storage classes
kubectl get sc | grep couchestor
```

### 2. Create Test Workload

```yaml
# test-workload.yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: test-openstack-pvc
  namespace: default
  annotations:
    couchestor.io/policy: openstack-tiered-policy
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
  name: test-openstack-pod
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
        claimName: test-openstack-pvc
```

```bash
kubectl apply -f test-workload.yaml

# Wait for pod
kubectl wait --for=condition=Ready pod/test-openstack-pod --timeout=300s

# Verify Cinder volume
openstack volume list | grep pvc

# Generate I/O
kubectl exec -it test-openstack-pod -- dd if=/dev/urandom of=/data/testfile bs=1M count=100
```

### 3. Test Migration

```bash
# Check current tier
kubectl get pvc test-openstack-pvc -o jsonpath='{.metadata.annotations}'

# Force migration to warm tier
kubectl annotate pvc test-openstack-pvc couchestor.io/force-tier=warm

# Monitor migration
kubectl get events -n default --field-selector involvedObject.name=test-openstack-pvc -w

# Verify Cinder volume type changed
openstack volume show $(kubectl get pv -o jsonpath='{.items[?(@.spec.claimRef.name=="test-openstack-pvc")].spec.csi.volumeHandle}')
```

---

## Troubleshooting

### Common Issues

#### 1. Cinder CSI Driver Issues

```bash
# Check CSI driver pods
kubectl -n kube-system get pods -l app=openstack-cinder-csi

# Check CSI driver logs
kubectl -n kube-system logs -l app=openstack-cinder-csi -c cinder-csi-plugin

# Verify cloud config
kubectl -n kube-system get secret cinder-csi-cloud-config -o jsonpath='{.data.cloud\.conf}' | base64 -d
```

#### 2. Authentication Failures

```bash
# Test OpenStack credentials
kubectl -n couchestor-system exec -it deploy/couchestor-controller -- bash -c '
  export OS_AUTH_URL=$OS_AUTH_URL
  export OS_APPLICATION_CREDENTIAL_ID=$OS_APPLICATION_CREDENTIAL_ID
  export OS_APPLICATION_CREDENTIAL_SECRET=$OS_APPLICATION_CREDENTIAL_SECRET
  openstack token issue
'

# Check credential expiration
openstack application credential show couchestor-cred
```

#### 3. Volume Stuck in Creating

```bash
# Check Cinder logs (on OpenStack side)
tail -f /var/log/cinder/cinder-volume.log

# Check volume status
openstack volume show <volume-id>

# Check scheduler logs
tail -f /var/log/cinder/cinder-scheduler.log
```

#### 4. Migration Failures

```bash
# Check controller logs for migration errors
kubectl -n couchestor-system logs -l component=controller | grep -i migration

# Verify volume can be retyped
openstack volume retype --migration-policy on-demand <volume-id> couchestor-warm

# Check for quota issues
openstack quota show --volume
```

### Debug Mode

```bash
# Enable debug logging
kubectl -n couchestor-system set env deploy/couchestor-controller COUCHESTOR_LOG_LEVEL=debug

# Collect diagnostics
kubectl -n couchestor-system logs -l app=couchestor --all-containers --timestamps > couchestor-debug.log
kubectl -n kube-system logs -l app=openstack-cinder-csi --all-containers >> couchestor-debug.log
```

---

## Appendix

### A. Heat Template for Infrastructure

```yaml
# couchestor-heat-template.yaml
heat_template_version: 2021-04-16
description: CoucheStor Infrastructure on OpenStack

parameters:
  key_name:
    type: string
  image:
    type: string
    default: ubuntu-22.04
  master_flavor:
    type: string
    default: m1.large
  worker_flavor:
    type: string
    default: m1.xlarge

resources:
  network:
    type: OS::Neutron::Net
    properties:
      name: couchestor-network

  subnet:
    type: OS::Neutron::Subnet
    properties:
      network: { get_resource: network }
      cidr: 10.10.0.0/24
      gateway_ip: 10.10.0.1

  router:
    type: OS::Neutron::Router
    properties:
      external_gateway_info:
        network: external-network

  router_interface:
    type: OS::Neutron::RouterInterface
    properties:
      router: { get_resource: router }
      subnet: { get_resource: subnet }

  k8s_cluster:
    type: OS::Magnum::Cluster
    properties:
      name: couchestor-cluster
      cluster_template: k8s-template
      master_count: 3
      node_count: 3
      keypair: { get_param: key_name }

outputs:
  cluster_uuid:
    value: { get_resource: k8s_cluster }
```

### B. Terraform Alternative

```hcl
# main.tf
terraform {
  required_providers {
    openstack = {
      source  = "terraform-provider-openstack/openstack"
      version = "~> 1.51"
    }
  }
}

resource "openstack_containerinfra_cluster_v1" "couchestor" {
  name                = "couchestor-cluster"
  cluster_template_id = openstack_containerinfra_clustertemplate_v1.k8s.id
  master_count        = 3
  node_count          = 3
  keypair             = var.keypair_name
}

resource "openstack_blockstorage_volume_type_v3" "hot" {
  name = "couchestor-hot"
  extra_specs = {
    volume_backend_name = "nvme-pool"
    couchestor_tier     = "hot"
  }
}
```

### C. Cleanup

```bash
# Delete test resources
kubectl delete -f test-workload.yaml

# Uninstall CoucheStor
kubectl delete -f couchestor-agent-openstack.yaml
kubectl delete -f couchestor-deployment-openstack.yaml
kubectl delete -f couchestor-config-openstack.yaml
kubectl delete -f storage-classes.yaml
kubectl delete -f deploy/crds/
kubectl delete namespace couchestor-system

# Remove OpenStack resources
openstack volume type delete couchestor-hot couchestor-warm couchestor-cold
openstack coe cluster delete couchestor-cluster
```

---

## Support

- **Documentation**: https://github.com/abiolaogu/couchestor/docs
- **Issues**: https://github.com/abiolaogu/couchestor/issues
- **OpenStack Integration**: https://github.com/abiolaogu/couchestor/docs/openstack

---

*Last Updated: February 2026*
*CoucheStor Version: 0.1.0*
*OpenStack Release: Zed / 2023.1+*
