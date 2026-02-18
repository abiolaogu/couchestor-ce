# Software Requirements — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Runtime Dependencies

### 1.1 Required Software
| Software | Version | Purpose |
|----------|---------|---------|
| Kubernetes | 1.28+ | Container orchestration platform |
| OpenEBS Mayastor | 2.x | CSI storage driver and volume management |
| Prometheus | 2.x | Metrics collection and PromQL queries |
| Container Runtime | containerd 1.7+ or CRI-O 1.28+ | Pod execution |
| Linux Kernel | 5.15+ | Host operating system |

### 1.2 Optional Software
| Software | Version | Purpose |
|----------|---------|---------|
| SPDK | 21.0+ | High-performance NVMe userspace driver |
| Intel ISA-L | 2.0+ | Hardware-accelerated erasure coding |
| DPDK | 21.0+ (bundled with SPDK) | Data Plane Development Kit |
| nvme-cli | 2.0+ | NVMe device management and SMART data |
| smartmontools | 7.3+ | SAS/SATA SMART data retrieval |
| Grafana | 9.0+ | Metrics visualization dashboards |
| AlertManager | 0.25+ | Alert routing and notification |

### 1.3 Build Dependencies
| Software | Version | Purpose |
|----------|---------|---------|
| Rust | 1.76+ | Compiler toolchain |
| Cargo | (bundled with Rust) | Build system and package manager |
| pkg-config | 0.3+ | Library discovery (optional) |
| gcc/clang | Any recent | C linker for native dependencies |
| Docker/Podman | 20.0+ / 4.0+ | Container image building |

## 2. Rust Crate Dependencies

### 2.1 Core Dependencies
| Crate | Version | License | Purpose |
|-------|---------|---------|---------|
| kube | 0.99 | Apache-2.0 | Kubernetes client and controller runtime |
| k8s-openapi | 0.24 | Apache-2.0 | Kubernetes API types (v1.32) |
| tokio | 1.36 | MIT | Async runtime (full features + tracing) |
| futures | 0.3 | MIT/Apache-2.0 | Async trait futures and combinators |
| async-trait | 0.1 | MIT/Apache-2.0 | Async trait support |
| serde | 1.0 | MIT/Apache-2.0 | Serialization framework |
| serde_json | 1.0 | MIT/Apache-2.0 | JSON serialization |
| serde_yaml | 0.9 | MIT/Apache-2.0 | YAML serialization |
| reqwest | 0.12 | MIT/Apache-2.0 | HTTP client (rustls-tls, json) |
| thiserror | 1.0 | MIT/Apache-2.0 | Error derive macros |
| anyhow | 1.0 | MIT/Apache-2.0 | Flexible error handling |

### 2.2 Observability Dependencies
| Crate | Version | License | Purpose |
|-------|---------|---------|---------|
| tracing | 0.1 | MIT | Structured logging framework |
| tracing-subscriber | 0.3 | MIT | Log formatting (env-filter, json) |
| prometheus | 0.14 | Apache-2.0 | Prometheus metrics exposition |
| hyper | 1.5 | MIT | HTTP server (metrics/health endpoints) |
| hyper-util | 0.1 | MIT | Hyper server utilities |
| http-body-util | 0.1 | MIT | HTTP body utilities |

### 2.3 Storage and Data Dependencies
| Crate | Version | License | Purpose |
|-------|---------|---------|---------|
| reed-solomon-erasure | 6.0 | MIT | Pure Rust Reed-Solomon codec |
| bytes | 1.5 | MIT | Zero-copy byte buffers |
| lz4 | 1.28 | MIT | LZ4 compression |
| parking_lot | 0.12 | MIT/Apache-2.0 | Fast synchronization primitives |
| dashmap | 6.1 | MIT | Lock-free concurrent hash maps |
| crossbeam | 0.8 | MIT/Apache-2.0 | Lock-free data structures |

### 2.4 Utility Dependencies
| Crate | Version | License | Purpose |
|-------|---------|---------|---------|
| chrono | 0.4 | MIT/Apache-2.0 | Date/time handling (with serde) |
| tokio-util | 0.7 | MIT | Tokio utility types |
| schemars | 0.8 | MIT | JSON Schema generation for CRDs |
| clap | 4.5 | MIT/Apache-2.0 | CLI argument parsing (derive, env) |
| urlencoding | 2.1 | MIT | URL encoding for Prometheus queries |
| once_cell | 1.19 | MIT/Apache-2.0 | Lazy static initialization |
| uuid | 1.8 | MIT/Apache-2.0 | UUID v4 generation |

### 2.5 Optional Dependencies
| Crate | Version | Feature | Purpose |
|-------|---------|---------|---------|
| libc | 0.2 | spdk | C FFI types for SPDK integration |
| pkg-config | 0.3 | build | Library discovery |

### 2.6 Development Dependencies
| Crate | Version | Purpose |
|-------|---------|---------|
| tokio-test | 0.4 | Async test utilities |
| assert_matches | 1.5 | Pattern matching assertions |
| proptest | 1.4 | Property-based testing |

## 3. Kubernetes API Requirements

### 3.1 API Groups Used
| API Group | Version | Resources |
|-----------|---------|-----------|
| storage.billyronks.io | v1 | storagepolicies, erasurecodingpolicies, ecstripes |
| openebs.io | v1beta2 | diskpools |
| openebs.io | v1alpha1 | mayastorvolumes |
| "" (core) | v1 | persistentvolumes, persistentvolumeclaims, events |
| storage.k8s.io | v1 | storageclasses |
| coordination.k8s.io | v1 | leases |
| rbac.authorization.k8s.io | v1 | clusterroles, clusterrolebindings |
| apps | v1 | deployments |

### 3.2 RBAC Permissions Required
```yaml
# StoragePolicy CRDs: full management
storage.billyronks.io/storagepolicies: get, list, watch, update, patch
storage.billyronks.io/storagepolicies/status: get, list, watch, update, patch

# EC Policy CRDs: full management
storage.billyronks.io/erasurecodingpolicies: get, list, watch, update, patch
storage.billyronks.io/erasurecodingpolicies/status: get, list, watch, update, patch

# Mayastor CRDs: read + modify for migrations
openebs.io/diskpools: get, list, watch, update, patch
openebs.io/mayastorvolumes: get, list, watch, update, patch

# Core resources: read-only for PV/PVC discovery
core/persistentvolumes: get, list, watch
core/persistentvolumeclaims: get, list, watch
core/events: create, patch

# Storage: read-only for StorageClass discovery
storage.k8s.io/storageclasses: get, list, watch

# Coordination: leader election
coordination.k8s.io/leases: get, create, update
```

## 4. Container Image Requirements

### 4.1 Base Image
- **Image**: `gcr.io/distroless/static-debian12:nonroot`
- **Size**: ~5 MB
- **Contains**: Only CA certificates and timezone data
- **No**: Shell, package manager, libc (uses static linking)

### 4.2 CoucheStor Binary
- **Name**: `couchestor`
- **Size**: ~8-10 MB (release, LTO, stripped)
- **Target**: `x86_64-unknown-linux-gnu`
- **Linking**: Static (for distroless compatibility)
- **Build**: `cargo zigbuild --release --target x86_64-unknown-linux-gnu`

### 4.3 Container Ports
| Port | Protocol | Purpose |
|------|----------|---------|
| 8080 | TCP/HTTP | Prometheus metrics endpoint |
| 8081 | TCP/HTTP | Health check endpoints |

## 5. Prometheus Configuration

### 5.1 Required Prometheus Scrape Config
```yaml
- job_name: 'couchestor'
  kubernetes_sd_configs:
    - role: service
      namespaces:
        names: ['couchestor-system']
  relabel_configs:
    - source_labels: [__meta_kubernetes_service_name]
      regex: couchestor-metrics
      action: keep
```

### 5.2 Required Mayastor Metrics
CoucheStor queries these metrics from Prometheus:
| Metric | Source | Query Pattern |
|--------|--------|--------------|
| openebs_volume_iops | Mayastor | rate(openebs_volume_iops{volume="{id}"}[1h]) |
| mayastor_volume_iops | Mayastor (fallback) | rate(mayastor_volume_iops{volume="{id}"}[1h]) |
| mayastor_volume_read_ops | Mayastor (fallback) | rate(mayastor_volume_read_ops{volume="{id}"}[1h]) |

## 6. Operating System Requirements

### 6.1 Supported Operating Systems
| OS | Version | Support Level |
|----|---------|--------------|
| Ubuntu | 22.04, 24.04 | Primary |
| RHEL/Rocky | 8.x, 9.x | Primary |
| Debian | 12 (Bookworm) | Primary |
| Fedora | 38+ | Secondary |
| SLES | 15 SP5+ | Secondary |

### 6.2 Kernel Requirements
| Feature | Minimum Kernel | Purpose |
|---------|---------------|---------|
| NVMe support | 4.x | NVMe device access |
| io_uring | 5.1+ | Async I/O (optional) |
| ZNS support | 5.9+ | Zoned Namespace NVMe |
| Hugepages | 2.6+ | SPDK DMA memory |
| VFIO | 4.x | SPDK userspace NVMe |

### 6.3 Required Kernel Modules
| Module | Required | Purpose |
|--------|----------|---------|
| nvme | For NVMe devices | NVMe driver |
| nvme-fabrics | For NVMe-oF | NVMe over Fabrics |
| vfio-pci | For SPDK only | Userspace device access |

## 7. Compatibility Matrix

| Component | Tested Versions | Notes |
|-----------|----------------|-------|
| Kubernetes | 1.28, 1.29, 1.30, 1.31, 1.32 | k8s-openapi targets v1.32 |
| OpenEBS Mayastor | 2.4, 2.5, 2.6 | CRD API v1beta2/v1alpha1 |
| Prometheus | 2.45+, 2.50+ | PromQL v2 API |
| Rust toolchain | 1.76, 1.77, 1.78, 1.79, 1.80 | Edition 2021 |
| Container runtime | containerd 1.7+, CRI-O 1.28+ | OCI compliant |

## 8. License Summary

All dependencies use permissive licenses (MIT, Apache-2.0, MIT/Apache-2.0 dual). No copyleft (GPL) dependencies in the dependency tree. CoucheStor CE itself is Apache-2.0 licensed.
