# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

CoucheStor Community Edition (CE) is a Kubernetes operator written in Rust for intelligent tiered storage with erasure coding support. It automatically migrates volumes between Hot, Warm, and Cold tiers based on IOPS metrics from Prometheus, with Reed-Solomon erasure coding for storage-efficient cold tier.

## Community Edition Features

| Feature | Included |
|---------|----------|
| Tiered Storage (Hot/Warm/Cold) | ✓ |
| Basic Erasure Coding (4+2) | ✓ |
| L1/L2/L3 Cache | ✓ |
| LZ4 Compression | ✓ |
| Hardware Discovery | ✓ |
| Prometheus Metrics | ✓ |
| Kubernetes CRDs | ✓ |

For Enterprise features (multi-tenancy, replication, audit, Zstd/Snappy compression, async prefetch), see CoucheStor Enterprise Edition.

## Build Commands

```bash
cargo build --release              # Build optimized binary
cargo build --features mock-spdk   # Build with mock SPDK for testing
cargo fmt                          # Format code
cargo clippy                       # Run lints
cargo test                         # Run all tests
cargo test --features mock-spdk    # Run all tests including SPDK modules
cargo test <test_name>             # Run a single test
cargo test -- --nocapture          # Run tests with stdout output
cargo doc --open                   # Generate and view documentation
```

## Architecture

The operator follows a three-component "Eyes, Brain, Hands" pattern:

```
Metrics Watcher (Eyes) → Controller (Brain) → Migrator (Hands)
     watcher.rs              main.rs            engine.rs
```

### Module Structure

```rust
mod adapters;     // DDD infrastructure adapters (Prometheus, Mayastor, K8s, etc.)
mod controller;   // Reconciliation loop
mod crd;          // CRD types (StoragePolicy, Mayastor types)
mod domain;       // DDD ports (traits) and domain events
mod ec;           // Erasure coding (metadata, stripe manager, reconstruction)
mod error;        // Error types (thiserror)
mod hardware;     // Hardware discovery (NVMe, SAS, SATA enumeration)
mod metrics;      // Re-exports MetricsWatcher
mod migrator;     // Re-exports Migrator
mod rustfs;       // High-performance object storage engine (cache, monitoring)
mod spdk;         // SPDK/ISA-L integration (feature-gated)
```

## RustFS - High-Performance Object Storage Engine

The `rustfs` module implements object storage capabilities using:
- **Data-Oriented Design (DOD)**: CPU cache optimization, zero-copy networking
- **Domain-Driven Design (DDD)**: Distinct domains for each feature
- **Test-Driven Development (TDD)**: Comprehensive test coverage

### Three-Tiered Cache System (`src/rustfs/cache/`)

```
┌──────────────────────────────────────────────────────────────────────────┐
│                        Cache Manager                                      │
├──────────────────────────────────────────────────────────────────────────┤
│  L1 Cache (RAM)       │ L2 Cache (NVMe)     │ L3 Cache (Cold Storage)   │
│  ┌────────────────┐   │ ┌────────────────┐  │ ┌────────────────────┐    │
│  │ ShardedHashMap │   │ │ MappedFile     │  │ │ Async Storage      │    │
│  │ (1024-way)     │   │ │ + Index        │  │ │ Backend            │    │
│  │ Capacity: 50GB │   │ │ Capacity: 500GB│  │ │ Capacity: 10TB+    │    │
│  └────────────────┘   │ └────────────────┘  │ └────────────────────┘    │
└──────────────────────────────────────────────────────────────────────────┘
```

**Compression**: LZ4 (fast compression)

**Performance Targets:**
- L1 Read: < 1μs latency, 2M ops/sec
- L1 Write: < 5μs latency, 500K ops/sec
- L2 Read: < 100μs latency, 500K ops/sec
- L3 Read: < 10ms latency, 10K ops/sec

### Production Observability (`src/rustfs/monitoring/`)

- `MetricsCollector`: Lock-free metrics registry (Counter, Gauge, Histogram)
- `HealthCheck`: Liveness/readiness probes (Healthy, Degraded, Unhealthy)

## Key Dependencies

- `kube` 0.99 - Kubernetes client & controller runtime
- `tokio` - Async runtime
- `reed-solomon-erasure` - Pure Rust Reed-Solomon codec
- `parking_lot` - Fast synchronization primitives
- `bytes` - Zero-copy byte buffers
- `lz4` - Fast compression

## Configuration

Environment variables:
- `PROMETHEUS_URL` - Prometheus server URL
- `MAX_CONCURRENT_MIGRATIONS` - Parallel migration limit (default: 2)
- `DRY_RUN` - Log only, no actual migrations
- `PRESERVATION_MODE` - Never remove old replicas
- `LOG_LEVEL` - trace/debug/info/warn/error

## Endpoints

- `:8080/metrics` - Prometheus metrics
- `:8081/healthz`, `:8081/livez` - Liveness probe
- `:8081/readyz` - Readiness probe

## Testing

```bash
cargo test                          # Run all tests
cargo test --lib rustfs::           # Run RustFS tests only
cargo test --lib ec::               # Run EC tests only
cargo test --test ec_integration    # Run EC integration tests
```
