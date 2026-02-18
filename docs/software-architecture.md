# Software Architecture Document — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Introduction

This document describes the software architecture of CoucheStor Community Edition, focusing on component decomposition, interfaces, data structures, and design decisions.

## 2. Architectural Style

CoucheStor employs a layered hexagonal architecture (ports and adapters) combined with the Kubernetes operator pattern.

```
┌─────────────────────────────────────────────────────────┐
│                    Presentation Layer                     │
│  CLI (clap) │ Health Server (hyper) │ Metrics Server     │
├─────────────────────────────────────────────────────────┤
│                   Application Layer                       │
│  Controller (reconciliation) │ StripeManager │ Migrator  │
├─────────────────────────────────────────────────────────┤
│                     Domain Layer                          │
│  Ports (traits) │ Events │ Value Objects                  │
├─────────────────────────────────────────────────────────┤
│                  Infrastructure Layer                     │
│  Adapters: Prometheus │ Mayastor │ K8s │ ReedSolomon     │
├─────────────────────────────────────────────────────────┤
│                    Platform Layer                         │
│  SPDK/ISA-L │ Tokio Runtime │ Kubernetes API │ Sysfs     │
└─────────────────────────────────────────────────────────┘
```

## 3. Component Specifications

### 3.1 Controller Component (`src/controller/`)

**Responsibility**: Kubernetes reconciliation loop for StoragePolicy and ErasureCodingPolicy CRDs.

**Files**:
- `storage_policy.rs`: Watches StoragePolicy resources, queries metrics, makes tiering decisions
- `ec_policy.rs`: Watches ErasureCodingPolicy resources, validates configurations

**Key Types**:
```rust
pub struct ControllerContext {
    client: Client,
    metrics_watcher: MetricsWatcher,
    migrator: Migrator,
    max_concurrent_migrations: usize,
}

pub struct EcPolicyContext {
    client: Client,
}
```

**Interfaces**:
- `run(ctx: ControllerContext) -> Result<()>` — Main StoragePolicy reconciliation loop
- `run_ec_policy(ctx: EcPolicyContext) -> Result<()>` — EC policy reconciliation loop

### 3.2 Metrics Watcher Component (`src/metrics/`)

**Responsibility**: Query Prometheus for volume IOPS metrics with caching and fallback.

**Configuration**:
```rust
pub struct MetricsConfig {
    prometheus_url: String,
    query_timeout: Duration,       // Default: 30s
    cache_enabled: bool,           // Default: true
    cache_ttl: Duration,           // Default: 30s
    metric_name: String,           // Default: "openebs_volume_iops"
    fallback_metrics: Vec<String>, // ["mayastor_volume_iops", ...]
}
```

**Interfaces**:
- `new(config: MetricsConfig) -> Result<Self>`
- `health_check(&self) -> Result<()>` — Verify Prometheus connectivity
- Implements `MetricsProvider` trait from domain layer

### 3.3 Migrator Component (`src/migrator/`)

**Responsibility**: Execute safe 4-step volume migrations between storage pools.

**Configuration**:
```rust
pub struct MigratorConfig {
    sync_timeout: Duration,        // Default: 30 minutes
    sync_poll_interval: Duration,  // Default: 10 seconds
    max_retries: u32,              // Default: 3
    dry_run: bool,
    preservation_mode: bool,
}
```

**Migration State Machine**:
```
 ┌─────────┐    ┌──────────┐    ┌───────────┐    ┌────────────┐
 │ ANALYZE  │───▶│ SCALE UP │───▶│ WAIT SYNC │───▶│ SCALE DOWN │
 └─────────┘    └──────────┘    └───────────┘    └────────────┘
      │              │               │                  │
      ▼              ▼               ▼                  ▼
  [Verify State] [Add Replica] [Poll Synced]    [Remove Old]
      │              │               │                  │
      └──────────────┴───────────────┴──────────────────┘
                           │
                      On Any Failure:
                   Preserve All Replicas
```

### 3.4 Erasure Coding Component (`src/ec/`)

**Sub-components**:

| File | Responsibility |
|------|---------------|
| `encoder.rs` | Reed-Solomon encode/decode using reed-solomon-erasure crate |
| `metadata.rs` | ECStripe CRD persistence, LBA-to-stripe mapping |
| `stripe_manager.rs` | Background destaging from journal to EC stripes |
| `reconstruction.rs` | Degraded reads, background rebuild, scrub verification |
| `proptest.rs` | Property-based testing for encoder correctness |

**Encoder Interface**:
```rust
// From domain/ports.rs EcCodec trait:
fn encode(&self, data: &[u8]) -> Result<EncodedData>;
fn decode(&self, shards: &mut [Option<Vec<u8>>], original_len: usize) -> Result<Vec<u8>>;
fn reconstruct(&self, shards: &mut [Option<Vec<u8>>]) -> Result<()>;
fn can_recover(&self, missing_count: usize) -> bool;
```

### 3.5 RustFS Cache Component (`src/rustfs/cache/`)

**Sub-components**:

| File | Responsibility |
|------|---------------|
| `shard.rs` | 1024-way ShardedMap for lock-free concurrent access |
| `l1.rs` | RAM-based L1 cache (50GB default, < 1us read latency) |
| `l2.rs` | NVMe-backed L2 cache (500GB default, memory-mapped files) |
| `l3.rs` | Cold storage L3 cache (10TB+, async backend trait) |
| `manager.rs` | Cache orchestrator: lookup L1 → L2 → L3, promotion/demotion |
| `entry.rs` | CacheEntry, CacheKey, EntryMetadata types |
| `compression.rs` | LZ4 compression (CE), algorithm abstraction |
| `policy.rs` | Eviction (LRU) and Promotion policies |
| `metrics.rs` | Cache hit/miss ratios, tier statistics |

**Key Types**:
```rust
pub struct CacheKey {
    bucket: String,
    key: String,
}

pub struct CacheEntry {
    data: Bytes,
    metadata: EntryMetadata,
}

pub enum CacheTier { L1, L2, L3 }

pub enum CompressionAlgorithm {
    None,
    Lz4,    // CE only
    // Zstd, Snappy — Enterprise only
}
```

### 3.6 SPDK Component (`src/spdk/`, feature-gated)

**Feature Flags**: `spdk` (real), `mock-spdk` (testing)

**Sub-components**:

| File | Responsibility |
|------|---------------|
| `ffi.rs` | Unsafe C FFI bindings to SPDK and ISA-L libraries |
| `dma_buf.rs` | DMA-aligned buffer pool for zero-copy I/O |
| `isal_codec.rs` | ISA-L hardware-accelerated EC encoding (AVX2/AVX-512) |
| `stripe_processor.rs` | Batch stripe encoding with DMA buffers |
| `ec_engine.rs` | Full EC storage engine with placement policies |
| `bdev.rs` | SPDK block device abstraction |
| `zns.rs` | Zoned Namespace SSD management |
| `read_path.rs` | EC read path with degraded read support |
| `destage_manager.rs` | Journal-to-EC destaging orchestration |
| `metadata_engine.rs` | WAL and checkpoint-based metadata persistence |
| `compression.rs` | SPDK acceleration for compression |
| `mock.rs` | Mock implementations for testing |

### 3.7 Hardware Discovery Component (`src/hardware/`)

**Sub-components**:

| File | Responsibility |
|------|---------------|
| `discovery/scanner.rs` | HardwareScanner with configurable sysfs paths |
| `discovery/nvme.rs` | NVMe controller/namespace enumeration, SMART data |
| `discovery/sas_sata.rs` | SAS/SATA device enumeration |
| `discovery/mod.rs` | DriveInfo, DriveType, NodeHardwareInfo types |

## 4. Interface Contracts

### 4.1 Domain Ports (Traits)

```rust
#[async_trait]
pub trait MetricsProvider: Send + Sync {
    async fn get_volume_iops(&self, volume_id: &VolumeId) -> Result<f64>;
    async fn get_heat_score(&self, volume_id: &VolumeId) -> Result<HeatScore>;
    async fn get_heat_scores(&self, volume_ids: &[VolumeId]) -> Result<Vec<(VolumeId, HeatScore)>>;
    async fn health_check(&self) -> Result<bool>;
}

#[async_trait]
pub trait VolumeManager: Send + Sync {
    async fn get_volume(&self, volume_id: &VolumeId) -> Result<Option<VolumeInfo>>;
    async fn list_volumes(&self) -> Result<Vec<VolumeInfo>>;
    async fn add_replica(&self, volume_id: &VolumeId, pool: &str) -> Result<ReplicaInfo>;
    async fn remove_replica(&self, volume_id: &VolumeId, replica_id: &str) -> Result<()>;
    async fn wait_replica_sync(&self, volume_id: &VolumeId, replica_id: &str, timeout: Duration) -> Result<bool>;
    async fn get_volume_tier(&self, volume_id: &VolumeId) -> Result<StorageTier>;
    async fn health_check(&self) -> Result<bool>;
}

#[async_trait]
pub trait EcCodec: Send + Sync {
    fn data_shards(&self) -> usize;
    fn parity_shards(&self) -> usize;
    fn encode(&self, data: &[u8]) -> Result<EncodedData>;
    fn decode(&self, shards: &mut [Option<Vec<u8>>], original_len: usize) -> Result<Vec<u8>>;
    fn reconstruct(&self, shards: &mut [Option<Vec<u8>>]) -> Result<()>;
}

#[async_trait]
pub trait StripeRepository: Send + Sync {
    async fn save(&self, stripe: &StripeMetadata) -> Result<()>;
    async fn find_by_id(&self, stripe_id: &StripeId) -> Result<Option<StripeMetadata>>;
    async fn find_by_lba(&self, volume_id: &VolumeId, lba: u64) -> Result<Option<StripeMetadata>>;
    async fn find_by_volume(&self, volume_id: &VolumeId) -> Result<Vec<StripeMetadata>>;
    async fn delete(&self, stripe_id: &StripeId) -> Result<()>;
}

#[async_trait]
pub trait EventPublisher: Send + Sync {
    async fn publish(&self, event: DomainEvent) -> Result<()>;
    async fn publish_all(&self, events: Vec<DomainEvent>) -> Result<()>;
}
```

### 4.2 HTTP Endpoints

| Endpoint | Port | Method | Response |
|----------|------|--------|----------|
| `/metrics` | 8080 | GET | Prometheus text format |
| `/healthz` | 8081 | GET | "ok" (200) or error (503) |
| `/livez` | 8081 | GET | "ok" (200) |
| `/readyz` | 8081 | GET | "ok" (200) |

### 4.3 Prometheus Metrics

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `storage_operator_reconcile_total` | Counter | - | Total reconciliations |
| `storage_operator_migrations_total` | Counter | status | Migration outcomes |
| `storage_operator_active_migrations` | Gauge | - | Currently running |
| `couchestor_ec_stripes_total` | Counter | - | Total EC stripes |

## 5. Data Structures

### 5.1 Value Objects (Domain Layer)

```rust
pub struct HeatScore { iops: f64, weighted_avg: f64, timestamp: DateTime<Utc> }
pub enum TierClassification { Hot, Warm, Cold }
pub enum StorageTier { Hot, Warm, Cold }
pub struct VolumeId(pub String);
pub struct StripeId(pub u64);
pub struct LbaRange { start: u64, end: u64 }
```

### 5.2 CRD Structures

**StoragePolicySpec**: highWatermarkIOPS (u32), warmWatermarkIOPS (u32), lowWatermarkIOPS (u32), samplingWindow (String), cooldownPeriod (String), hotPoolSelector, warmPoolSelector, coldPoolSelector (LabelSelector), ecPolicyRef (Option<String>), enabled (bool), dryRun (bool)

**ErasureCodingPolicySpec**: dataShards (u8, default 4), parityShards (u8, default 2), stripeSizeBytes (u64, default 1MB), algorithm (ReedSolomon|LRC), journalConfig (Optional), scrubbingEnabled (bool)

**ECStripeSpec**: volumeRef (String), stripeId (u64), policyRef (String), shardLocations (Vec<ShardLocation>), lbaRange (LbaRange), checksum (Option<String>), generation (u64)

## 6. Design Decisions

### 6.1 ADR-001: Rust for the Operator
**Decision**: Write the operator in Rust instead of Go.
**Rationale**: Memory safety without GC pauses, ~10MB binary size (vs ~50MB Go), native performance for EC encoding, type safety for complex state machines.
**Consequences**: Smaller community than Go for K8s operators, requires Rust expertise.

### 6.2 ADR-002: DDD with Ports and Adapters
**Decision**: Use hexagonal architecture with trait-based dependency injection.
**Rationale**: Testability (mock adapters), swappable backends, clean separation of concerns.
**Consequences**: More boilerplate for trait definitions, but enables comprehensive testing.

### 6.3 ADR-003: CRD-Based Metadata Storage
**Decision**: Store EC stripe metadata as Kubernetes CRDs (ECStripe).
**Rationale**: No external database dependency, leverages K8s etcd for persistence, standard kubectl access.
**Consequences**: etcd 1MB object limit, potential performance issues at very high stripe counts.

### 6.4 ADR-004: Feature-Gated SPDK
**Decision**: Make SPDK/ISA-L integration optional via Cargo features.
**Rationale**: Most users do not have SPDK installed; pure Rust RS works for moderate workloads.
**Consequences**: Two code paths to maintain, but mock-spdk feature enables full testing.

### 6.5 ADR-005: 4-Step Safe Migration
**Decision**: Always add new replica before removing old.
**Rationale**: Zero data loss guarantee; old replica preserved on any failure.
**Consequences**: Temporary doubled storage during migration, but safety is paramount.

## 7. Build System

### 7.1 Cargo Configuration
- Edition: 2021
- Release profile: LTO enabled, codegen-units=1, panic=abort, strip=true
- Dual target: library (`src/lib.rs`) and binary (`src/main.rs`)

### 7.2 build.rs
Handles SPDK/ISA-L/DPDK library linking when `spdk` feature is enabled:
- Searches standard library paths and environment-configured directories
- Links: libspdk_env_dpdk, libspdk_util, libspdk_log, libspdk_json, libspdk_thread
- Links: librte_eal, librte_mempool, librte_ring, librte_malloc (DPDK)
- Links: libisal (ISA-L erasure coding)
- Links: libnuma, libuuid, libpthread, libdl (system)

### 7.3 Feature Matrix
| Feature | Dependencies | Purpose |
|---------|-------------|---------|
| default | (none) | Basic operator without SPDK |
| spdk | libc | Real SPDK/ISA-L integration |
| mock-spdk | (none) | Mock SPDK for testing |
| ec-sidecar | spdk | Legacy alias |
