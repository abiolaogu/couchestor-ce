# CoucheStor Installation Guides

This directory contains comprehensive installation guides for deploying CoucheStor on various hyperconverged and cloud infrastructure platforms.

## Platform-Specific Guides

| Platform | Guide | Description |
|----------|-------|-------------|
| Harvester HCI | [HARVESTER_HCI.md](HARVESTER_HCI.md) | SUSE Harvester hyperconverged infrastructure with Longhorn |
| OpenStack | [OPENSTACK.md](OPENSTACK.md) | OpenStack cloud with Cinder/Manila storage backends |
| Nutanix | [NUTANIX.md](NUTANIX.md) | Nutanix AHV with NKE and Nutanix Volumes/Files/Objects |
| CloudStack | [CLOUDSTACK.md](CLOUDSTACK.md) | Apache CloudStack with CKS or self-managed Kubernetes |

## Quick Comparison

| Feature | Harvester | OpenStack | Nutanix | CloudStack |
|---------|-----------|-----------|---------|------------|
| Kubernetes | Built-in (RKE2) | Magnum/Manual | NKE/Manual | CKS/Manual |
| Block Storage | Longhorn | Cinder | Nutanix Volumes | CloudStack CSI |
| File Storage | Longhorn RWX | Manila | Nutanix Files | NFS |
| Object Storage | External S3 | Swift/Ceph | Nutanix Objects | S3-compatible |
| Native EC | No | Backend-dependent | Yes (AOS) | No |
| Live Migration | Yes | Yes | Yes (vMotion) | Yes |
| Multi-Zone | Limited | Yes | Yes | Yes |

## Prerequisites (All Platforms)

### Minimum Requirements

- Kubernetes 1.27+
- 3+ nodes for HA
- 16 GB RAM per node (minimum)
- 100 GB storage per node (minimum)
- 10 GbE networking (recommended)

### Required Tools

```bash
# kubectl
curl -LO "https://dl.k8s.io/release/$(curl -L -s https://dl.k8s.io/release/stable.txt)/bin/linux/amd64/kubectl"
chmod +x kubectl && sudo mv kubectl /usr/local/bin/

# Helm
curl https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash

# CoucheStor CRDs
git clone https://github.com/abiolaogu/couchestor.git
kubectl apply -f couchestor/deploy/crds/
```

## Generic Installation Steps

While each platform has specific requirements, the general installation flow is:

1. **Prepare Infrastructure**
   - Deploy Kubernetes cluster
   - Configure storage backends
   - Set up networking

2. **Install CSI Driver**
   - Platform-specific CSI driver
   - Create tiered storage classes

3. **Deploy CoucheStor**
   - Create namespace and secrets
   - Apply CRDs
   - Deploy controller and agents

4. **Configure Policies**
   - Create storage policies
   - Set up monitoring
   - Configure alerts

5. **Verify Installation**
   - Create test volumes
   - Verify tier migrations
   - Check metrics

## Storage Tier Design

CoucheStor uses a three-tier storage model across all platforms:

```
┌────────────────────────────────────────────────────────────────────┐
│                     Storage Tier Architecture                       │
├────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌──────────────┐                                                  │
│  │   HOT TIER   │  NVMe/SSD  │  High IOPS (>100)  │  7-day data   │
│  │   (L1/L2)    │  Fast      │  Active workloads  │  Replicated   │
│  └──────────────┘                                                  │
│         │                                                          │
│         │ Demotion (IOPS < 100)                                    │
│         ▼                                                          │
│  ┌──────────────┐                                                  │
│  │  WARM TIER   │  SSD/SAS   │  Medium IOPS       │  30-day data  │
│  │              │  Balanced  │  Less active       │  Replicated   │
│  └──────────────┘                                                  │
│         │                                                          │
│         │ Demotion (IOPS < 10)                                     │
│         ▼                                                          │
│  ┌──────────────┐                                                  │
│  │  COLD TIER   │  HDD/Object│  Low IOPS          │  Archive      │
│  │   (EC)       │  Capacity  │  Rarely accessed   │  Erasure Coded│
│  └──────────────┘                                                  │
│                                                                     │
└────────────────────────────────────────────────────────────────────┘
```

## Getting Help

- **Documentation**: https://github.com/abiolaogu/couchestor/docs
- **Issues**: https://github.com/abiolaogu/couchestor/issues
- **Discussions**: https://github.com/abiolaogu/couchestor/discussions

## Document Versions

| Document | Version | Last Updated |
|----------|---------|--------------|
| HARVESTER_HCI.md | 1.0.0 | 2026-02-04 |
| OPENSTACK.md | 1.0.0 | 2026-02-04 |
| NUTANIX.md | 1.0.0 | 2026-02-04 |
| CLOUDSTACK.md | 1.0.0 | 2026-02-04 |
