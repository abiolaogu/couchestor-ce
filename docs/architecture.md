# Architecture Document — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Architecture Overview

CoucheStor CE follows a three-component "Eyes, Brain, Hands" architecture pattern, implemented as a Kubernetes operator using Domain-Driven Design (DDD) with ports and adapters.

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    CoucheStor Community Edition                         │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  ┌──────────────┐    ┌──────────────────┐    ┌──────────────┐          │
│  │   Metrics    │    │   Controller     │    │   Migrator   │          │
│  │   Watcher    │───▶│   (Brain)        │───▶│   (Hands)    │          │
│  │   (Eyes)     │    │   Reconciliation │    │              │          │
│  └──────────────┘    └──────────────────┘    └──────────────┘          │
│         │                    │                       │                  │
│         ▼                    ▼                       ▼                  │
│    Prometheus          StoragePolicy           Mayastor CRDs           │
│    (metrics)              (CRD)              (volumes, pools)          │
│                              │                                         │
│                    ┌─────────┴──────────┐                              │
│                    │  ErasureCoding     │                              │
│                    │  Policy (CRD)      │                              │
│                    └────────────────────┘                              │
│                              │                                         │
│         ┌────────────────────┼────────────────────┐                    │
│         ▼                    ▼                    ▼                    │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐             │
│  │   Encoder    │    │   Stripe     │    │ Reconstruction│             │
│  │   /Decoder   │    │   Manager    │    │    Engine     │             │
│  └──────────────┘    └──────────────┘    └──────────────┘             │
│                                                                         │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │                    RustFS Object Storage Engine                   │   │
│  │  ┌────────┐  ┌────────┐  ┌────────┐  ┌──────────┐  ┌────────┐  │   │
│  │  │L1 Cache│  │L2 Cache│  │L3 Cache│  │Monitoring│  │Hardware│  │   │
│  │  │ (RAM)  │  │(NVMe)  │  │(Cold)  │  │(Metrics) │  │Discovery│  │   │
│  │  └────────┘  └────────┘  └────────┘  └──────────┘  └────────┘  │   │
│  └─────────────────────────────────────────────────────────────────┘   │
│                                                                         │
│  ┌─────────────────────────────────────────────────────────────────┐   │
│  │              SPDK/ISA-L Integration (Feature-Gated)              │   │
│  │  ┌────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐          │   │
│  │  │DMA Buf │  │ISA-L     │  │Stripe    │  │ZNS       │          │   │
│  │  │Pool    │  │Codec     │  │Processor │  │Manager   │          │   │
│  │  └────────┘  └──────────┘  └──────────┘  └──────────┘          │   │
│  └─────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────┘
```

## 2. Design Principles

### 2.1 Domain-Driven Design (DDD)
The codebase follows hexagonal architecture with clearly defined ports (traits in `domain/ports.rs`) and adapters (implementations in `adapters/`).

**Domain Layer** (`src/domain/`):
- `ports.rs`: Defines traits — MetricsProvider, VolumeManager, EcCodec, StripeRepository, EventPublisher
- `events.rs`: Defines 23 domain event types covering volume, migration, EC, and health events

**Infrastructure Layer** (`src/adapters/`):
- `prometheus.rs`: Implements MetricsProvider for Prometheus queries
- `mayastor.rs`: Implements VolumeManager for Mayastor API
- `kubernetes.rs`: Implements StripeRepository via K8s CRDs
- `reed_solomon.rs`: Implements EcCodec using reed-solomon-erasure crate
- `event_publisher.rs`: Implements EventPublisher for domain event distribution

### 2.2 Data-Oriented Design (DOD)
The RustFS cache system uses DOD principles:
- 1024-way sharded hash maps for lock-free concurrent access
- Cache-line aligned data structures (64 bytes)
- Zero-copy byte buffers via the `bytes` crate
- Memory-mapped files for L2 NVMe cache (kernel page cache utilization)

### 2.3 Safety-First Design
- Migration uses a 4-step ANALYZE → SCALE UP → WAIT SYNC → SCALE DOWN process
- Old replicas are never removed until new replicas are confirmed synced
- Preservation mode option prevents any replica removal
- All errors are typed via thiserror with structured context

## 3. Module Architecture

### 3.1 Module Dependency Graph
```
main.rs
  ├── controller/
  │     ├── storage_policy.rs    → domain/, adapters/, crd/
  │     └── ec_policy.rs         → ec/, crd/
  ├── metrics/
  │     └── watcher.rs           → adapters/prometheus
  ├── migrator/
  │     └── engine.rs            → adapters/mayastor, crd/
  ├── ec/
  │     ├── encoder.rs           → reed-solomon-erasure
  │     ├── metadata.rs          → crd/, kube
  │     ├── stripe_manager.rs    → encoder, metadata
  │     └── reconstruction.rs    → encoder, metadata
  ├── crd/
  │     ├── storage_policy.rs    → kube derive macros
  │     ├── erasure_coding.rs    → kube derive macros
  │     └── mayastor.rs          → kube derive macros
  ├── domain/
  │     ├── ports.rs             → async_trait (no dependencies)
  │     └── events.rs            → chrono, serde
  ├── adapters/
  │     ├── prometheus.rs        → reqwest, domain/ports
  │     ├── mayastor.rs          → kube, domain/ports
  │     ├── kubernetes.rs        → kube, domain/ports
  │     ├── reed_solomon.rs      → reed-solomon-erasure, domain/ports
  │     └── event_publisher.rs   → domain/events
  ├── rustfs/
  │     ├── cache/               → bytes, parking_lot, lz4
  │     └── monitoring/          → prometheus, lock-free atomics
  ├── hardware/
  │     └── discovery/           → tokio::process, sysfs
  └── spdk/ (feature-gated)
        ├── ffi.rs               → libc (unsafe)
        ├── dma_buf.rs           → ffi.rs
        ├── isal_codec.rs        → ffi.rs
        ├── stripe_processor.rs  → isal_codec, dma_buf
        ├── ec_engine.rs         → stripe_processor
        ├── bdev.rs              → ffi.rs
        ├── zns.rs               → bdev
        ├── read_path.rs         → ec_engine, bdev
        ├── destage_manager.rs   → stripe_processor
        ├── metadata_engine.rs   → ec_engine
        └── compression.rs       → lz4
```

### 3.2 CRD API Design

**API Group**: `storage.billyronks.io/v1`

| CRD | Scope | Short Names | Purpose |
|-----|-------|-------------|---------|
| StoragePolicy | Cluster | sp, spolicy | Defines tiering thresholds and pool selectors |
| ErasureCodingPolicy | Cluster | ecp | Defines EC configuration (k, m, stripe size) |
| ECStripe | Cluster | ecs | Tracks individual stripe metadata |

**Mayastor CRDs Used** (API Group: `openebs.io`):
| CRD | Version | Purpose |
|-----|---------|---------|
| DiskPool | v1beta2 | Storage pool management |
| MayastorVolume | v1alpha1 | Volume lifecycle and replicas |

## 4. Data Flow Architecture

### 4.1 Tiering Decision Flow
```
1. MetricsWatcher polls Prometheus every samplingWindow
2. For each volume, query: rate(openebs_volume_iops[1h])
3. Controller classifies volume using HeatScore:
   - IOPS >= highWatermark → Hot
   - lowWatermark < IOPS < highWatermark → Warm
   - IOPS <= lowWatermark → Cold
4. If classification differs from current tier:
   a. Check cooldown period
   b. Check concurrent migration limit
   c. Execute migration via Migrator
5. Migrator performs 4-step safe migration:
   a. ANALYZE: Verify current state and target pool availability
   b. SCALE UP: Add replica on target pool via Mayastor API
   c. WAIT SYNC: Poll replica status until Online + Synced
   d. SCALE DOWN: Remove old replica (unless preservation mode)
```

### 4.2 Erasure Coding Data Flow
```
1. Volume migrates to Cold tier
2. StoragePolicy references ErasureCodingPolicy (ecPolicyRef)
3. StripeManager reads volume data in stripe-sized chunks (default 1MB)
4. EcEncoder splits each stripe into k data shards
5. EcEncoder computes m parity shards using Reed-Solomon
6. Shards distributed across pools using ShardLocation metadata
7. ECStripe CRD created to track stripe-to-pool mapping
8. On read: Check ECStripe for shard locations → read k shards → return data
9. On shard failure: ReconstructionEngine reads k-1 data + parity → reconstruct
```

## 5. Concurrency Model

### 5.1 Async Runtime
- Tokio multi-threaded runtime with tracing integration
- Controller reconciliation loop runs as async stream
- EC background tasks (StripeManager, ReconstructionEngine) run as spawned tasks
- Health and metrics servers run as independent tasks

### 5.2 Synchronization
- L1 cache: parking_lot RwLock per shard (1024 shards = minimal contention)
- DashMap for concurrent hash maps in adapters
- Crossbeam for lock-free data structures
- Atomic operations for metrics counters

## 6. Error Handling Strategy

The `error.rs` module defines 25+ error variants organized by subsystem:
- Kubernetes API errors (from kube crate)
- Prometheus connection/query/parse errors
- Migration lifecycle errors (in progress, failed, timeout, no pool)
- Erasure coding errors (encoding, reconstruction, insufficient shards)
- SPDK/DMA errors (allocation, init, bdev, ISA-L)
- RustFS errors (config, tenant, quota, rate limit, replication)
- Hardware discovery errors (NVMe command, SMART data)
- Compression errors (compression/decompression failures)

All errors implement `thiserror::Error` and `std::fmt::Display` for structured logging.

## 7. Configuration Architecture

### 7.1 CLI Arguments (clap derive)
- `--prometheus-url`: Prometheus endpoint
- `--max-concurrent-migrations`: Parallel migration limit
- `--migration-timeout-minutes`: Per-migration timeout
- `--dry-run`: Log-only mode
- `--preservation-mode`: Never remove replicas
- `--metrics-addr`: Metrics server bind address
- `--health-addr`: Health server bind address
- `--log-level`: Logging verbosity
- `--log-json`: JSON log format

### 7.2 Environment Variable Overrides
Every CLI argument supports `env` attribute via clap, allowing configuration through:
- Kubernetes ConfigMap/Secret mounted as environment variables
- Direct container environment variables in Deployment spec

## 8. Deployment Architecture

### 8.1 Kubernetes Resources
```
Namespace: couchestor-system
├── ServiceAccount: couchestor-operator
├── ClusterRole: couchestor-operator
├── ClusterRoleBinding: couchestor-operator
├── Deployment: couchestor-operator (1 replica)
│     ├── Container: operator (distroless/static-debian12:nonroot)
│     ├── Port 8080: Prometheus metrics
│     └── Port 8081: Health probes
├── Service: couchestor-metrics (ClusterIP, port 8080)
└── Service: couchestor-health (ClusterIP, port 8081)
```

### 8.2 Container Security
- Base image: gcr.io/distroless/static-debian12:nonroot
- Runs as UID 65534 (nobody)
- Read-only root filesystem
- All capabilities dropped
- No privilege escalation
- Resource limits: 500m CPU, 512Mi memory

## 9. Technology Stack

| Layer | Technology | Version | Purpose |
|-------|-----------|---------|---------|
| Language | Rust | 1.76+ | System programming, memory safety |
| Async Runtime | Tokio | 1.36 | Concurrent I/O, task scheduling |
| K8s Client | kube | 0.99 | API interactions, CRD management |
| K8s Types | k8s-openapi | 0.24 | Kubernetes type definitions |
| EC Codec | reed-solomon-erasure | 6.0 | Reed-Solomon encoding/decoding |
| Compression | lz4 | 1.28 | Fast compression for cache/EC |
| HTTP Client | reqwest | 0.12 | Prometheus API queries |
| HTTP Server | hyper | 1.5 | Metrics and health endpoints |
| Metrics | prometheus | 0.14 | Metrics exposition format |
| CLI | clap | 4.5 | Command-line argument parsing |
| Serialization | serde/serde_json | 1.0 | JSON/YAML serialization |
| Logging | tracing | 0.1 | Structured, async-aware logging |
| Sync | parking_lot | 0.12 | Fast mutexes and RwLocks |
| Concurrent Maps | dashmap | 6.1 | Lock-free concurrent hash maps |
| Schema | schemars | 0.8 | CRD OpenAPI schema generation |
