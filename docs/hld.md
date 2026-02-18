# High-Level Design — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. System Overview

CoucheStor CE is a Kubernetes operator that implements intelligent tiered storage with erasure coding. The system continuously monitors volume IOPS via Prometheus and automatically migrates volumes between Hot (NVMe), Warm (SAS/SATA SSD), and Cold (HDD) storage tiers.

## 2. High-Level Component Diagram

```
┌───────────────────────────────────────────────────────────────────────┐
│                         Kubernetes Cluster                             │
│                                                                        │
│  ┌──────────────────────────────────────────────────────────────────┐ │
│  │                  CoucheStor Operator Pod                          │ │
│  │                                                                    │ │
│  │  ┌────────────┐  ┌─────────────┐  ┌────────────┐  ┌──────────┐  │ │
│  │  │  Metrics   │  │ Controller  │  │  Migrator  │  │   EC     │  │ │
│  │  │  Watcher   │  │ Reconciler  │  │  Engine    │  │  Module  │  │ │
│  │  └─────┬──────┘  └──────┬──────┘  └─────┬──────┘  └────┬─────┘  │ │
│  │        │                │                │              │         │ │
│  │  ┌─────┴────────────────┴────────────────┴──────────────┴─────┐  │ │
│  │  │                    Shared Infrastructure                     │  │ │
│  │  │  Domain Layer │ Adapters │ CRDs │ RustFS Cache │ Hardware  │  │ │
│  │  └────────────────────────────────────────────────────────────┘  │ │
│  │                                                                    │ │
│  │  ┌─────────────┐  ┌─────────────┐                                │ │
│  │  │ Metrics Srv │  │ Health Srv  │                                │ │
│  │  │  :8080      │  │  :8081      │                                │ │
│  │  └─────────────┘  └─────────────┘                                │ │
│  └──────────────────────────────────────────────────────────────────┘ │
│                                                                        │
│  ┌─────────┐  ┌─────────────┐  ┌──────────────────────────────────┐  │
│  │Prometheus│  │Mayastor     │  │       Storage Infrastructure     │  │
│  │  :9090   │  │Engine+CSI   │  │  NVMe Pools │ SAS Pools │ HDDs  │  │
│  └─────────┘  └─────────────┘  └──────────────────────────────────┘  │
└───────────────────────────────────────────────────────────────────────┘
```

## 3. Component Responsibilities

### 3.1 Metrics Watcher (Eyes)
- Queries Prometheus for volume IOPS using PromQL range queries
- Caches results with 30-second TTL to reduce load
- Supports fallback metric names for compatibility
- Computes HeatScore (time-weighted average) per volume

### 3.2 Controller (Brain)
- Runs Kubernetes reconciliation loop using kube-runtime
- Watches StoragePolicy CRDs for changes
- Classifies volumes into Hot/Warm/Cold tiers based on IOPS thresholds
- Enforces cooldown periods and concurrent migration limits
- Updates StoragePolicy status subresource

### 3.3 Migrator (Hands)
- Executes 4-step safe migration process
- Interacts with Mayastor API to add/remove replicas
- Polls replica sync status until confirmed
- Supports dry-run and preservation modes

### 3.4 EC Module
- **Encoder/Decoder**: Reed-Solomon encoding via reed-solomon-erasure crate
- **Metadata Manager**: Manages ECStripe CRDs in Kubernetes
- **Stripe Manager**: Background destaging from journal to EC-encoded cold storage
- **Reconstruction Engine**: Rebuilds degraded stripes, supports degraded reads

### 3.5 RustFS Cache
- **L1 (RAM)**: 1024-way sharded hash map, 50GB, sub-microsecond reads
- **L2 (NVMe)**: Memory-mapped files, 500GB, sub-100us reads
- **L3 (Cold)**: Async backend trait, 10TB+, sub-10ms reads
- **Compression**: LZ4 for cache entries and EC stripes

### 3.6 SPDK Integration (Optional)
- DMA-aligned buffer management for zero-copy I/O
- ISA-L hardware-accelerated EC encoding (AVX2/AVX-512)
- NVMe block device abstraction
- ZNS (Zoned Namespace) SSD management
- Feature-gated: `--features spdk` or `--features mock-spdk`

## 4. Data Flow Summary

### 4.1 Tiering Decision Path
```
Prometheus ─metrics─▶ Watcher ─HeatScore─▶ Controller ─decision─▶ Migrator ─API─▶ Mayastor
```

### 4.2 EC Encoding Path
```
Volume Data ─read─▶ StripeManager ─LZ4─▶ Encoder ─RS─▶ Shards ─distribute─▶ Pools
```

### 4.3 EC Read Path
```
Read Request ─LBA lookup─▶ MetadataManager ─locate─▶ Shards ─read─▶ Reconstruct ─return─▶ Caller
```

## 5. Technology Choices

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Language | Rust 1.76+ | Memory safety, ~10MB binary, native performance |
| Async runtime | Tokio | Industry standard, tracing integration |
| K8s client | kube 0.99 | Official Rust K8s client with controller runtime |
| EC codec | reed-solomon-erasure 6.0 | Pure Rust, no native dependencies |
| Compression | LZ4 | Fast compression/decompression (CE) |
| Metrics | Prometheus exposition | Industry standard, native integration |
| Sync primitives | parking_lot | Faster than std RwLock, smaller footprint |
| Container | Distroless | Minimal attack surface, ~5MB base |

## 6. Deployment Model

### 6.1 Kubernetes Resources
- **Namespace**: `couchestor-system`
- **Deployment**: 1 replica (single active controller)
- **ServiceAccount**: `couchestor-operator` with ClusterRole
- **Services**: metrics (8080), health (8081)
- **CRDs**: StoragePolicy, ErasureCodingPolicy, ECStripe

### 6.2 Resource Requirements
| Resource | Request | Limit |
|----------|---------|-------|
| CPU | 100m | 500m |
| Memory | 128Mi | 512Mi |
| Disk | None (stateless) | None |

## 7. Security Model

- Container runs as nonroot (UID 65534)
- Read-only root filesystem
- All capabilities dropped
- No privilege escalation
- ClusterRole with minimal required permissions
- No secrets stored (uses ServiceAccount token mounting)

## 8. Failure Handling

| Failure | Handling |
|---------|---------|
| Prometheus down | Operator continues, logs warnings, no migrations triggered |
| Mayastor API error | Migration aborted, old replica preserved |
| Replica sync timeout | Migration aborted, both replicas preserved |
| Shard failure (< m) | Degraded read + background reconstruction |
| Shard failure (> m) | Stripe marked Failed, alert emitted |
| Operator crash | K8s restarts, reconciliation resumes from CRD state |
| etcd unavailable | All K8s API calls fail, operator blocks until recovery |

## 9. Scalability Considerations

| Factor | Current | Scaling Strategy |
|--------|---------|-----------------|
| Volumes | 1000+ | Batched Prometheus queries, cached metrics |
| EC Stripes | 100K+ | CRD-based, etcd scales with cluster |
| Migration speed | 2 concurrent (configurable) | Mayastor-limited I/O bandwidth |
| Cache capacity | L1:50G L2:500G L3:10T+ | Tier overflow with eviction |
| Controller memory | ~128Mi base | Linear with watched volume count |

## 10. Monitoring and Observability

### 10.1 Prometheus Metrics
- `storage_operator_reconcile_total`: Reconciliation count
- `storage_operator_migrations_total{status}`: Migration outcomes
- `storage_operator_active_migrations`: In-flight migrations

### 10.2 Health Endpoints
- `/healthz`, `/livez`: Liveness (operator process alive)
- `/readyz`: Readiness (K8s client connected, Prometheus reachable)

### 10.3 Logging
- Structured logging via tracing crate
- Configurable levels: trace, debug, info, warn, error
- JSON format for production log aggregation
- Log context includes volume IDs, tier classifications, migration states
