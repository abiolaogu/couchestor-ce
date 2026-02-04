# CoucheStor

[![Rust](https://img.shields.io/badge/Rust-1.76+-orange?logo=rust)](https://www.rust-lang.org)
[![Kubernetes](https://img.shields.io/badge/Kubernetes-1.28+-326CE5?logo=kubernetes)](https://kubernetes.io)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

**Intelligent tiered storage operator with erasure coding** — written in Rust for maximum performance and safety.

## Features

- **Multi-tier storage** — Hot, Warm, and Cold tiers with automatic migration based on IOPS
- **Erasure coding** — Reed-Solomon EC for storage-efficient cold tier (e.g., 4+2 = 50% overhead vs 200% for 3-way replication)
- **Kubernetes-native** — CRDs for StoragePolicy and ErasureCodingPolicy
- **Safe migrations** — State machine ensures data is never lost during tier transitions

## Why Rust?

| Aspect | Benefit |
|--------|---------|
| **Memory Safety** | Zero-cost abstractions, no garbage collector pauses |
| **Performance** | Native speed, minimal resource footprint (~10MB binary) |
| **Reliability** | Compile-time guarantees prevent entire classes of bugs |
| **Type Safety** | Strong typing catches errors at compile time, not runtime |

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                           CoucheStor                               │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐       │
│  │   Metrics    │    │  Controller  │    │   Migrator   │       │
│  │   Watcher    │───▶│    (Brain)   │───▶│   (Hands)    │       │
│  │   (Eyes)     │    │  Reconciler  │    │              │       │
│  └──────────────┘    └──────────────┘    └──────────────┘       │
│         │                   │                    │               │
│         ▼                   ▼                    ▼               │
│    Prometheus         StoragePolicy       Mayastor CRDs          │
│     (metrics)            (CRD)            (volumes, pools)       │
│                             │                                    │
│                             ▼                                    │
│                    ErasureCodingPolicy                           │
│                         (CRD)                                    │
└─────────────────────────────────────────────────────────────────┘
```

## Quick Start

### Prerequisites

- Rust 1.76+ (`rustup update stable`)
- Kubernetes 1.28+
- [OpenEBS Mayastor](https://mayastor.gitbook.io/)
- Prometheus with Mayastor metrics

### Build & Run

```bash
# Build release binary
cargo build --release

# Run locally (with port-forwarded Prometheus)
kubectl port-forward svc/prometheus 9090:9090 -n monitoring &
cargo run -- --prometheus-url=http://localhost:9090

# Build & push Docker image
docker build -t myregistry/couchestor:v1.0.0 .
docker push myregistry/couchestor:v1.0.0
```

### Deploy to Kubernetes

```bash
# Install CRDs
kubectl apply -f deploy/crds/

# Deploy operator
kubectl apply -f deploy/operator.yaml

# Create a policy
kubectl apply -f deploy/examples/storagepolicy-examples.yaml

# Check status
kubectl get storagepolicies
```

## Project Structure

```
src/
├── main.rs              # Entry point, CLI, servers
├── error.rs             # Error types (thiserror)
├── crd/
│   ├── mod.rs           # CRD exports
│   ├── storage_policy.rs # StoragePolicy CRD
│   ├── erasure_coding.rs # ErasureCodingPolicy CRD
│   └── mayastor.rs      # Mayastor CRD mirrors
├── metrics/
│   └── watcher.rs       # Prometheus query client
├── migrator/
│   └── engine.rs        # Safe migration state machine
├── controller/
│   ├── storage_policy.rs # Reconciliation logic
│   └── ec_policy.rs     # EC policy controller
└── ec/
    ├── encoder.rs       # Reed-Solomon encoding
    ├── metadata.rs      # LBA-to-stripe mapping
    ├── stripe_manager.rs # Journal destaging
    └── reconstruction.rs # Degraded reads
```

## Configuration

### Command-Line Options

```
USAGE:
    couchestor [OPTIONS]

OPTIONS:
    --prometheus-url <URL>
        Prometheus server URL [default: http://prometheus.monitoring.svc.cluster.local:9090]

    --max-concurrent-migrations <N>
        Maximum parallel migrations [default: 2]

    --migration-timeout-minutes <N>
        Timeout per migration [default: 30]

    --dry-run
        Log migrations without executing

    --preservation-mode
        Never remove old replicas (safest)

    --log-level <LEVEL>
        trace, debug, info, warn, error [default: info]

    --log-json
        Output logs as JSON

    --metrics-addr <ADDR>
        Metrics endpoint [default: 0.0.0.0:8080]

    --health-addr <ADDR>
        Health endpoint [default: 0.0.0.0:8081]
```

### Environment Variables

All CLI options can be set via environment variables:

```bash
PROMETHEUS_URL=http://prometheus:9090
MAX_CONCURRENT_MIGRATIONS=2
MIGRATION_TIMEOUT_MINUTES=30
DRY_RUN=true
LOG_LEVEL=debug
```

## StoragePolicy CRD

```yaml
apiVersion: storage.billyronks.io/v1
kind: StoragePolicy
metadata:
  name: production-tiering
spec:
  highWatermarkIOPS: 5000    # → Hot tier when exceeded
  warmWatermarkIOPS: 2000    # → Warm tier threshold
  lowWatermarkIOPS: 500      # → Cold tier when below
  samplingWindow: "1h"       # IOPS averaging window
  cooldownPeriod: "24h"      # Anti-thrashing delay
  storageClassName: "mayastor"

  hotPoolSelector:
    matchLabels:
      storage-tier: hot

  warmPoolSelector:
    matchLabels:
      storage-tier: warm

  coldPoolSelector:
    matchLabels:
      storage-tier: cold

  # Erasure coding for cold tier
  ecPolicyRef: standard-ec
  ecMinVolumeSizeBytes: 10737418240  # 10GB minimum for EC

  enabled: true
```

## ErasureCodingPolicy CRD

```yaml
apiVersion: storage.billyronks.io/v1
kind: ErasureCodingPolicy
metadata:
  name: standard-ec
spec:
  dataShards: 4
  parityShards: 2
  stripeSizeBytes: 1048576  # 1MB
  algorithm: ReedSolomon
  journalConfig:
    journalSizeBytes: 10737418240  # 10GB
    replicationFactor: 3
    destageThresholdPercent: 80
```

## Safety Guarantees

### Migration Safety
```
1. ANALYZE    → Verify current state
2. SCALE UP   → Add replica on target pool
3. WAIT SYNC  → Poll until Online AND Synced
4. SCALE DOWN → Remove old replica ONLY if sync succeeded
```

Data is **never lost** because:
- Old replica preserved if sync fails
- Old replica preserved on timeout
- Old replica preserved on any error
- Preservation mode option never removes old replicas

### Erasure Coding Safety
- Reed-Solomon encoding tolerates up to `m` shard failures (e.g., 4+2 tolerates 2 failures)
- Degraded reads transparently reconstruct missing data
- Background rebuild restores full redundancy

## Observability

### Metrics (`:8080/metrics`)

```
couchestor_reconcile_total
couchestor_migrations_total{status="success|failed|aborted"}
couchestor_active_migrations
couchestor_ec_stripes_total
couchestor_ec_reconstructions_total
```

### Health Endpoints (`:8081`)

- `/healthz` - Liveness probe
- `/readyz` - Readiness probe

### Logs

```bash
# Stream logs
kubectl logs -n couchestor-system -l app.kubernetes.io/name=couchestor -f

# Debug level
RUST_LOG=debug ./couchestor
```

## Development

### Commands

```bash
# Format code
cargo fmt

# Run lints
cargo clippy

# Run tests
cargo test

# Generate docs
cargo doc --open

# Build release
cargo build --release
```

### Testing Locally

```bash
# 1. Port-forward Prometheus
kubectl port-forward svc/prometheus 9090:9090 -n monitoring &

# 2. Run with debug logging
RUST_LOG=debug cargo run -- \
    --prometheus-url=http://localhost:9090 \
    --dry-run
```

## RustFS - Enterprise Object Storage

CoucheStor includes **RustFS**, a high-performance object storage engine with enterprise features:

### Three-Tiered Cache System

```rust
use couchestor::rustfs::cache::{CacheManager, CacheKey, CacheEntry};
use bytes::Bytes;

// Create cache manager with in-memory L3 backend (for testing)
let manager = CacheManager::in_memory();

// Put an object
let key = CacheKey::new("my-bucket", "docs/report.pdf");
let data = Bytes::from_static(b"PDF content here...");
let entry = CacheEntry::new(data);
manager.put(key.clone(), entry).await?;

// Get an object (searches L1→L2→L3 automatically)
if let Some(result) = manager.get(&key).await {
    println!("Found in: {}", result.tier);  // L1 (RAM), L2 (NVMe), or L3 (Cold)
    println!("Latency: {:?}", result.latency);
}

// View metrics
let metrics = manager.metrics();
println!("L1 hit ratio: {:.2}%", (metrics.l1_hits as f64 / (metrics.l1_hits + metrics.l1_misses) as f64) * 100.0);
println!("Total cache size: {} bytes", metrics.l1_size + metrics.l2_size);
```

### Active-Active Multi-Region Replication

```rust
use couchestor::rustfs::replication::{
    ReplicationManager, ReplicationConfig, ReplicationMode, ReplicationEvent,
};

// Create replication manager with semi-sync mode
let config = ReplicationConfig {
    mode: ReplicationMode::SemiSync { min_ack: 2 },  // Wait for 2 regions
    ..Default::default()
};
let manager = ReplicationManager::new(config, connector);

// Queue replication events
manager.queue_event(ReplicationEvent::put("bucket", "key", 1024));
manager.flush().await?;

// Check replication stats
println!("Replicated events: {}", manager.events_replicated());
println!("Replication lag: {:?}", manager.replication_lag());
```

### Enterprise Multi-Tenancy

```rust
use couchestor::rustfs::tenancy::{TenantManager, Tenant, TenantId, TenantConfig};

// Create tenant manager
let manager = TenantManager::new(TenantConfig::default());

// Create a tenant with custom quotas
let tenant = Tenant::new("tenant-123", "Acme Corp")
    .with_storage_quota(1024 * 1024 * 1024 * 1024)  // 1TB
    .with_object_quota(10_000_000)                  // 10M objects
    .with_rps_quota(5000);                          // 5K RPS
manager.create_tenant(tenant)?;

// Check quota before write
let id = TenantId::new("tenant-123");
let result = manager.check_write(&id, 1024);
if result.is_allowed() {
    // Perform write
    manager.record_write(&id, 1024);
}
```

### Production Observability

```rust
use couchestor::rustfs::monitoring::{MetricsCollector, HealthCheck};

// Metrics collection
let collector = MetricsCollector::default_config();
collector.counter("requests_total").inc();
collector.gauge("active_connections").set(42);
collector.histogram("request_latency_seconds").observe(0.05);

// Export Prometheus format
let prometheus_text = collector.export_text();

// Health checks
let health = HealthCheck::new();
health.set_ready(true);
let status = health.check_all();
```

### RustFS Features

| Feature | Description | Status |
|---------|-------------|--------|
| **L1 Cache (RAM)** | 1024-way sharded hashmap, < 1μs latency | ✅ Production |
| **L2 Cache (NVMe)** | Memory-mapped index, < 100μs latency | ✅ Production |
| **L3 Cache (Cold)** | Async storage backend, < 10ms latency | ✅ Production |
| **Replication** | Multi-region active-active with conflict resolution | ✅ Production |
| **Tenancy** | Per-tenant quotas and rate limiting | ✅ Production |
| **Monitoring** | Prometheus metrics and health checks | ✅ Production |

**Test Coverage:** 193+ tests covering all RustFS modules

## License

Apache License 2.0
