# Hardware Requirements — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Overview

CoucheStor CE is a lightweight Kubernetes operator. Its hardware requirements depend on the storage infrastructure it manages. This document covers requirements for the operator itself and for the storage nodes it orchestrates.

## 2. Operator Node Requirements

### 2.1 Minimum Requirements
| Resource | Minimum | Recommended |
|----------|---------|-------------|
| CPU | 1 vCPU | 2 vCPU |
| Memory | 256 MB | 512 MB |
| Disk | 100 MB (binary + logs) | 1 GB |
| Network | 1 Gbps | 10 Gbps |
| Architecture | x86_64 | x86_64 |

### 2.2 Operator Resource Limits (Kubernetes)
```yaml
resources:
  requests:
    cpu: 100m
    memory: 128Mi
  limits:
    cpu: 500m
    memory: 512Mi
```

### 2.3 Scaling Factors
| Factor | Memory Impact | CPU Impact |
|--------|-------------|-----------|
| Per 100 watched volumes | +10 MB | +5% of 1 core |
| Per active migration | +5 MB | +10% of 1 core |
| Metrics cache (per 1000 entries) | +2 MB | Negligible |
| EC metadata manager | +20 MB base | +5% per 10K stripes |

## 3. Storage Node Requirements

### 3.1 Hot Tier (NVMe)
| Component | Minimum | Recommended |
|-----------|---------|-------------|
| Drive Type | NVMe SSD (PCIe 3.0 x4) | NVMe SSD (PCIe 4.0 x4 or PCIe 5.0) |
| IOPS (4K Random Read) | 100,000 | 500,000+ |
| Throughput (Sequential) | 2 GB/s | 7+ GB/s |
| Endurance | 1 DWPD | 3+ DWPD |
| Capacity per Drive | 480 GB | 1.92 TB - 7.68 TB |
| Drives per Node | 1 | 2-4 |
| Form Factor | U.2, M.2 | U.2, E1.S, E3.S |
| Interface | NVMe 1.3+ | NVMe 1.4+ (ZNS support) |

**Recommended NVMe Models**:
- Intel Optane P5800X (ultra-low latency)
- Samsung PM9A3 (balanced performance/capacity)
- Kioxia CM7 (PCIe 5.0)
- Solidigm D7-P5620 (high endurance)

### 3.2 Warm Tier (SAS/SATA SSD)
| Component | Minimum | Recommended |
|-----------|---------|-------------|
| Drive Type | SATA SSD | SAS SSD (12 Gbps) |
| IOPS (4K Random Read) | 50,000 | 200,000+ |
| Throughput (Sequential) | 500 MB/s | 2+ GB/s (SAS) |
| Endurance | 1 DWPD | 3+ DWPD |
| Capacity per Drive | 960 GB | 1.92 TB - 7.68 TB |
| Drives per Node | 2 | 4-8 |

### 3.3 Cold Tier (HDD)
| Component | Minimum | Recommended |
|-----------|---------|-------------|
| Drive Type | SATA HDD (7200 RPM) | SAS HDD (7200 RPM) or CMR |
| IOPS (4K Random Read) | 100 | 200+ |
| Throughput (Sequential) | 150 MB/s | 250+ MB/s |
| Capacity per Drive | 4 TB | 16 TB - 22 TB |
| Drives per Node | 4 (minimum for 4+2 EC) | 8-12 |
| Notes | Avoid SMR drives for EC workloads | CMR (Conventional Magnetic Recording) preferred |

**Recommended HDD Models**:
- Seagate Exos X20 (20TB, CMR)
- WD Ultrastar HC570 (22TB, CMR)
- Toshiba MG10 (20TB, CMR)

### 3.4 Node Configuration for EC (4+2)
For erasure coding with 4 data + 2 parity shards, each shard should be on a different node/drive for optimal fault tolerance:
- **Minimum**: 6 drives across 3+ nodes
- **Recommended**: 6+ drives across 6 nodes (one shard per node)
- **Storage overhead**: 1.5x (6 shards for 4 shards of data)

## 4. Network Requirements

| Requirement | Minimum | Recommended |
|-------------|---------|-------------|
| Node-to-Node | 10 Gbps | 25 Gbps |
| Storage Network | Dedicated VLAN | Dedicated NIC + RDMA |
| Latency (intra-cluster) | < 1 ms | < 0.5 ms |
| MTU | 1500 | 9000 (Jumbo Frames) |

### 4.1 Bandwidth Calculation for Migration
A 10 GB volume migration at 10 Gbps network:
- Theoretical: 10 GB / (10 Gbps / 8) = 8 seconds
- Practical (with overhead): ~15-30 seconds
- At 25 Gbps: ~6-12 seconds

### 4.2 EC Shard Distribution Bandwidth
Encoding a 1 MB stripe into 6 shards (4+2) and writing to 6 nodes:
- Per shard: 256 KB (1 MB / 4, rounded up)
- Total writes: 6 x 256 KB = 1.5 MB
- At 10 Gbps: ~0.12 ms per stripe distribution

## 5. Kubernetes Cluster Requirements

### 5.1 Control Plane
| Component | Minimum | Recommended |
|-----------|---------|-------------|
| Kubernetes Version | 1.28 | 1.30+ |
| etcd | 3 nodes | 5 nodes (for large EC deployments) |
| API Server | 2 vCPU, 4 GB | 4 vCPU, 8 GB |

### 5.2 Worker Nodes
| Component | Minimum (3 nodes) | Recommended (6+ nodes) |
|-----------|-------------------|----------------------|
| CPU | 4 vCPU | 16+ vCPU |
| Memory | 16 GB | 64+ GB |
| OS Disk | 100 GB SSD | 200+ GB SSD |
| Hugepages (SPDK) | Not required | 1 GB 2MB hugepages |

## 6. SPDK-Specific Requirements (Optional)

When using the `spdk` feature flag:
| Component | Requirement |
|-----------|-------------|
| Hugepages (2MB) | 1 GB minimum, 4+ GB recommended |
| IOMMU | Enabled in BIOS and kernel |
| vfio-pci | Kernel module loaded |
| SPDK libraries | v21.0+ installed |
| ISA-L libraries | v2.0+ installed |
| CPU | AVX2 minimum, AVX-512 recommended |
| DPDK | Bundled with SPDK |

### 6.1 SPDK Performance Impact
| Operation | Without SPDK | With SPDK (AVX-512) | Speedup |
|-----------|-------------|---------------------|---------|
| RS Encode 1MB | ~5 ms | ~0.3 ms | ~17x |
| RS Decode 1MB | ~5 ms | ~0.3 ms | ~17x |
| NVMe Read 4KB | ~10 us (kernel) | ~2 us (userspace) | ~5x |
| NVMe Write 4KB | ~15 us (kernel) | ~3 us (userspace) | ~5x |

## 7. Reference Architectures

### 7.1 Small Deployment (Dev/Test)
```
3 Nodes:
  Each: 4 vCPU, 16 GB RAM
  Hot: 1x 480 GB NVMe per node
  Cold: 2x 4 TB HDD per node
  Network: 10 Gbps
  EC: 4+2 across 6 drives
```
**Total**: 1.44 TB hot, 24 TB cold (16 TB usable with EC)

### 7.2 Medium Deployment (Production)
```
6 Nodes:
  Each: 16 vCPU, 64 GB RAM
  Hot: 2x 1.92 TB NVMe per node
  Warm: 2x 3.84 TB SAS SSD per node
  Cold: 4x 16 TB HDD per node
  Network: 25 Gbps
  EC: 4+2 with one shard per node
```
**Total**: 23 TB hot, 46 TB warm, 384 TB cold (256 TB usable)

### 7.3 Large Deployment (Enterprise-Scale)
```
12+ Nodes:
  Each: 32 vCPU, 128 GB RAM, 1 GB hugepages
  Hot: 4x 3.84 TB NVMe per node (with SPDK)
  Warm: 4x 7.68 TB SAS SSD per node
  Cold: 8x 22 TB HDD per node
  Network: 100 Gbps RDMA
  EC: 8+3 or 10+4 for larger stripe groups
```
**Total**: 184 TB hot, 369 TB warm, 2.1 PB cold (1.5+ PB usable)

## 8. Environmental Requirements

| Factor | Requirement |
|--------|-------------|
| Power | Standard data center (per node: 500-1500W depending on drive count) |
| Cooling | Standard data center air cooling |
| Rack Space | Standard 1U-2U per node |
| Operating System | Linux (kernel 5.15+, recommended 6.x for ZNS) |
| Filesystem | XFS or ext4 for Mayastor pools |
