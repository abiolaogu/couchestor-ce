# CoucheStor Community Edition

[![Rust](https://img.shields.io/badge/Rust-1.76+-orange?logo=rust)](https://www.rust-lang.org)
[![Kubernetes](https://img.shields.io/badge/Kubernetes-1.28+-326CE5?logo=kubernetes)](https://kubernetes.io)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Tests](https://img.shields.io/badge/Tests-34%20passing-brightgreen)]()

**Intelligent tiered storage operator with erasure coding** — written in Rust for maximum performance and safety.

> **Looking for Enterprise features?** Multi-tenancy, replication, and audit logging are available in [CoucheStor Enterprise Edition](https://github.com/abiolaogu/couchestor-ee).

## Features

| Feature | Community Edition |
|---------|:-----------------:|
| Tiered Storage (Hot/Warm/Cold) | ✓ |
| Erasure Coding (4+2 Reed-Solomon) | ✓ |
| L1/L2/L3 Cache System | ✓ |
| LZ4 Compression | ✓ |
| Hardware Discovery (NVMe/SAS/SATA) | ✓ |
| Prometheus Metrics | ✓ |
| Kubernetes CRDs | ✓ |
| Health Checks | ✓ |

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
│                    CoucheStor Community Edition                 │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐      │
│  │   Metrics    │    │  Controller  │    │   Migrator   │      │
│  │   Watcher    │───▶│    (Brain)   │───▶│   (Hands)    │      │
│  │   (Eyes)     │    │  Reconciler  │    │              │      │
│  └──────────────┘    └──────────────┘    └──────────────┘      │
│         │                   │                    │              │
│         ▼                   ▼                    ▼              │
│    Prometheus         StoragePolicy       Mayastor CRDs        │
│     (metrics)            (CRD)            (volumes, pools)     │
│                             │                                   │
│                             ▼                                   │
│                    ErasureCodingPolicy                          │
│                         (CRD)                                   │
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

## Configuration

### Environment Variables

```bash
PROMETHEUS_URL=http://prometheus:9090
MAX_CONCURRENT_MIGRATIONS=2
MIGRATION_TIMEOUT_MINUTES=30
DRY_RUN=true
LOG_LEVEL=debug
```

### Command-Line Options

```
couchestor [OPTIONS]

OPTIONS:
    --prometheus-url <URL>           Prometheus server URL [default: http://prometheus.monitoring.svc.cluster.local:9090]
    --max-concurrent-migrations <N>  Maximum parallel migrations [default: 2]
    --dry-run                        Log migrations without executing
    --preservation-mode              Never remove old replicas (safest)
    --log-level <LEVEL>              trace, debug, info, warn, error [default: info]
    --metrics-addr <ADDR>            Metrics endpoint [default: 0.0.0.0:8080]
    --health-addr <ADDR>             Health endpoint [default: 0.0.0.0:8081]
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
  samplingWindow: "1h"
  cooldownPeriod: "24h"

  hotPoolSelector:
    matchLabels:
      storage-tier: hot
  coldPoolSelector:
    matchLabels:
      storage-tier: cold

  ecPolicyRef: standard-ec
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
```

## Three-Tiered Cache System

```rust
use couchestor::rustfs::cache::{CacheManager, CacheKey, CacheEntry};
use bytes::Bytes;

let manager = CacheManager::in_memory();

// Put an object
let key = CacheKey::new("my-bucket", "docs/report.pdf");
let entry = CacheEntry::new(Bytes::from("PDF content"));
manager.put(key.clone(), entry).await?;

// Get (searches L1→L2→L3 automatically)
if let Some(result) = manager.get(&key).await {
    println!("Found in: {}", result.tier);  // L1 (RAM), L2 (NVMe), or L3 (Cold)
}

// Metrics
let metrics = manager.metrics();
println!("Hit ratio: {:.2}%", metrics.overall_hit_ratio * 100.0);
```

### Cache Performance Targets

| Tier | Latency | Throughput |
|------|---------|------------|
| L1 (RAM) | < 1μs | 2M ops/sec |
| L2 (NVMe) | < 100μs | 500K ops/sec |
| L3 (Cold) | < 10ms | 10K ops/sec |

## Safety Guarantees

### Migration Safety

```
1. ANALYZE    → Verify current state
2. SCALE UP   → Add replica on target pool
3. WAIT SYNC  → Poll until Online AND Synced
4. SCALE DOWN → Remove old replica ONLY if sync succeeded
```

Data is **never lost** because old replicas are preserved on any failure.

### Erasure Coding Safety

- Reed-Solomon encoding tolerates up to `m` shard failures
- 4+2 configuration = 50% overhead (vs 200% for 3-way replication)
- Degraded reads transparently reconstruct missing data

## Observability

### Metrics (`:8080/metrics`)

```
couchestor_reconcile_total
couchestor_migrations_total{status="success|failed|aborted"}
couchestor_active_migrations
couchestor_ec_stripes_total
```

### Health Endpoints (`:8081`)

- `/healthz` - Liveness probe
- `/readyz` - Readiness probe

## Development

```bash
cargo fmt                  # Format code
cargo clippy               # Run lints
cargo test                 # Run tests (34 integration tests)
cargo build --release      # Build release binary
```

## Upgrade to Enterprise

Need multi-tenancy, replication, or audit logging? Upgrade to [CoucheStor Enterprise Edition](https://github.com/abiolaogu/couchestor-ee).

See [EDITIONS.md](EDITIONS.md) for a full feature comparison.

## License

Apache License 2.0 - See [LICENSE](LICENSE) for details.

---

**Copyright (c) 2024 BillyRonks Global Limited**
