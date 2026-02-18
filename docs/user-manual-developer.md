# User Manual: Developer — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Introduction

This manual is for developers contributing to CoucheStor CE or integrating with its Rust library. It covers the development environment setup, code structure, testing, and API usage.

## 2. Development Environment Setup

### 2.1 Prerequisites
```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update stable
rustc --version  # Requires 1.76+

# Install development tools
cargo install cargo-watch    # Auto-rebuild on changes
cargo install cargo-audit    # Security vulnerability scanning
cargo install cargo-tarpaulin # Code coverage (Linux)
```

### 2.2 Clone and Build
```bash
git clone https://github.com/abiolaogu/couchestor-ce.git
cd couchestor-ce

# Build (default, no SPDK)
cargo build

# Build release
cargo build --release

# Build with mock SPDK (enables SPDK module tests)
cargo build --features mock-spdk
```

### 2.3 Run Tests
```bash
# All tests
cargo test

# All tests including SPDK mock modules
cargo test --features mock-spdk

# Specific test module
cargo test --lib ec::
cargo test --lib rustfs::
cargo test --lib crd::

# Integration tests
cargo test --test ec_integration
cargo test --test integration_tests

# With output
cargo test -- --nocapture

# Single test
cargo test test_parse_duration_hours
```

### 2.4 Code Quality
```bash
# Format code
cargo fmt

# Run lints
cargo clippy

# Generate documentation
cargo doc --open

# Security audit
cargo audit
```

## 3. Project Structure

```
couchestor-ce/
├── src/
│   ├── main.rs              # Binary entry point (CLI, servers, controller startup)
│   ├── lib.rs               # Library root (public API, re-exports)
│   ├── error.rs             # Error types (25+ variants with thiserror)
│   ├── adapters/            # Infrastructure adapters (DDD)
│   │   ├── prometheus.rs    # MetricsProvider implementation
│   │   ├── mayastor.rs      # VolumeManager implementation
│   │   ├── kubernetes.rs    # StripeRepository implementation
│   │   ├── reed_solomon.rs  # EcCodec implementation
│   │   └── event_publisher.rs # EventPublisher implementation
│   ├── controller/          # K8s reconciliation loops
│   │   ├── storage_policy.rs # StoragePolicy controller
│   │   └── ec_policy.rs     # ErasureCodingPolicy controller
│   ├── crd/                 # CRD type definitions
│   │   ├── storage_policy.rs # StoragePolicy + status + label selectors
│   │   ├── erasure_coding.rs # ErasureCodingPolicy + ECStripe
│   │   └── mayastor.rs      # DiskPool + MayastorVolume
│   ├── domain/              # Domain layer (DDD ports + events)
│   │   ├── ports.rs         # Traits: MetricsProvider, VolumeManager, etc.
│   │   └── events.rs        # 23 domain event types
│   ├── ec/                  # Erasure coding
│   │   ├── encoder.rs       # Reed-Solomon encode/decode
│   │   ├── metadata.rs      # ECStripe CRD management
│   │   ├── stripe_manager.rs # Background destaging
│   │   ├── reconstruction.rs # Degraded reads + rebuild
│   │   └── proptest.rs      # Property-based tests
│   ├── metrics/             # Prometheus integration
│   │   └── watcher.rs       # IOPS query + caching
│   ├── migrator/            # Volume migration
│   │   └── engine.rs        # 4-step safe migration
│   ├── rustfs/              # Object storage engine
│   │   ├── cache/           # Three-tiered cache
│   │   │   ├── shard.rs     # 1024-way ShardedMap
│   │   │   ├── l1.rs        # RAM cache
│   │   │   ├── l2.rs        # NVMe cache (mmap)
│   │   │   ├── l3.rs        # Cold storage backend
│   │   │   ├── manager.rs   # Cache orchestrator
│   │   │   ├── entry.rs     # CacheKey, CacheEntry
│   │   │   ├── compression.rs # LZ4 compression
│   │   │   ├── policy.rs    # Eviction/promotion policies
│   │   │   └── metrics.rs   # Hit/miss statistics
│   │   └── monitoring/      # Observability
│   │       ├── collector.rs # Lock-free metrics (Counter, Gauge, Histogram)
│   │       └── health.rs    # Liveness/readiness probes
│   ├── hardware/            # Hardware discovery
│   │   └── discovery/       # NVMe/SAS/SATA enumeration
│   └── spdk/                # SPDK/ISA-L (feature-gated)
│       ├── ffi.rs           # Unsafe C FFI bindings
│       ├── dma_buf.rs       # DMA buffer pool
│       ├── isal_codec.rs    # ISA-L accelerated EC
│       └── ... (12 files)
├── tests/
│   ├── ec_integration.rs    # EC encode/decode/reconstruct
│   └── integration_tests.rs # Controller integration
├── deploy/
│   ├── crds/                # CRD YAML manifests
│   ├── examples/            # Example CRD instances
│   └── operator.yaml        # K8s deployment manifest
├── Cargo.toml               # Dependencies and features
├── build.rs                 # SPDK/ISA-L link configuration
└── Dockerfile               # Distroless container image
```

## 4. Library API Reference

### 4.1 Using CoucheStor as a Library
Add to your `Cargo.toml`:
```toml
[dependencies]
couchestor = { path = "../couchestor-ce" }
```

### 4.2 Cache API
```rust
use couchestor::rustfs::cache::*;
use bytes::Bytes;

// Create an in-memory cache manager
let config = CacheConfig::default();
let manager = CacheManager::new(config);

// Create a cache key
let key = CacheKey::new("bucket-name", "object/key/path");

// Create a cache entry with metadata
let data = Bytes::from(vec![0u8; 1024]);
let entry = CacheEntry::new(data);

// Put and get
manager.put(key.clone(), entry).await?;
let result = manager.get(&key).await;

// Compression
let compressor = CompressionManager::new(CompressionAlgorithm::Lz4);
let compressed = compressor.compress(&data)?;
let decompressed = compressor.decompress(&compressed)?;
```

### 4.3 Erasure Coding API
```rust
use couchestor::ec::encoder::{EcEncoder, EcDecoder};

// Create encoder (4 data + 2 parity)
let encoder = EcEncoder::new(4, 2)?;

// Encode data
let data = b"Hello, World! This is some test data for erasure coding.";
let shards = encoder.encode(data)?;
// shards.data_shards: Vec<Vec<u8>> (4 shards)
// shards.parity_shards: Vec<Vec<u8>> (2 shards)

// Simulate shard loss
let mut optional_shards: Vec<Option<Vec<u8>>> = shards.into_iter().map(Some).collect();
optional_shards[0] = None; // Lose shard 0
optional_shards[3] = None; // Lose shard 3

// Reconstruct
let decoder = EcDecoder::new(4, 2)?;
let recovered = decoder.decode(&mut optional_shards, data.len())?;
assert_eq!(recovered, data);
```

### 4.4 Domain Events API
```rust
use couchestor::domain::events::DomainEvent;
use couchestor::domain::ports::StorageTier;
use std::time::Duration;

// Create events using builder methods
let event = DomainEvent::migration_started(
    "pvc-123",
    StorageTier::Hot,
    StorageTier::Cold,
    "pool-nvme-1",
    "pool-hdd-1",
);

// Serialize to JSON
let json = serde_json::to_string(&event)?;

// Access event properties
println!("Type: {}", event.event_type());
println!("Volume: {:?}", event.volume_id());
println!("Timestamp: {}", event.timestamp());
```

### 4.5 CRD Types API
```rust
use couchestor::crd::{StoragePolicy, ErasureCodingPolicy};

// StoragePolicy provides helper methods
let policy: StoragePolicy = /* from K8s API */;
assert!(policy.is_enabled());
assert!(!policy.is_dry_run());
let window = policy.sampling_window()?; // std::time::Duration
let cooldown = policy.cooldown_period()?;
assert!(policy.warm_tier_enabled());
assert!(policy.ec_enabled());

// ErasureCodingPolicy validation
let ec_policy: ErasureCodingPolicy = /* from K8s API */;
ec_policy.validate()?; // Returns Result<(), String>
let efficiency = ec_policy.storage_efficiency(); // 0.667 for 4+2
let overhead = ec_policy.storage_overhead(); // 1.5 for 4+2
```

## 5. Adding New Features

### 5.1 Adding a New Domain Port
1. Define trait in `src/domain/ports.rs`:
```rust
#[async_trait]
pub trait MyNewPort: Send + Sync {
    async fn do_something(&self, id: &str) -> Result<()>;
}
```

2. Create adapter in `src/adapters/my_adapter.rs`:
```rust
pub struct MyAdapter { /* ... */ }

#[async_trait]
impl MyNewPort for MyAdapter {
    async fn do_something(&self, id: &str) -> Result<()> {
        // Implementation
    }
}
```

3. Add to `src/adapters/mod.rs`

### 5.2 Adding a New CRD
1. Define spec/status in `src/crd/my_resource.rs` using `#[derive(CustomResource)]`
2. Add to `src/crd/mod.rs` and re-export types
3. Create CRD YAML in `deploy/crds/`
4. Add RBAC rules to `deploy/operator.yaml`

### 5.3 Adding a New Error Variant
Add to `src/error.rs`:
```rust
#[error("My new error: {0}")]
MyNewError(String),
```

## 6. Testing Patterns

### 6.1 Unit Test Pattern
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_my_function() {
        let result = my_function(input);
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_async_function() {
        let result = my_async_function().await;
        assert!(result.is_ok());
    }
}
```

### 6.2 Property-Based Testing (EC)
```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_encode_decode_roundtrip(data in proptest::collection::vec(any::<u8>(), 1..10000)) {
        let encoder = EcEncoder::new(4, 2).unwrap();
        let shards = encoder.encode(&data).unwrap();
        // ... test reconstruction
    }
}
```

## 7. Debugging

### 7.1 Enable Trace Logging
```bash
RUST_LOG=trace cargo run -- --prometheus-url=http://localhost:9090
```

### 7.2 Run with Mock SPDK
```bash
cargo run --features mock-spdk -- --prometheus-url=http://localhost:9090
```

### 7.3 Generate Documentation
```bash
cargo doc --open --document-private-items
```

## 8. Release Process

1. Update version in `Cargo.toml`
2. Run full test suite: `cargo test --features mock-spdk`
3. Run clippy: `cargo clippy -- -D warnings`
4. Build release: `cargo build --release`
5. Build Docker: `docker build -t couchestor:vX.Y.Z .`
6. Tag release: `git tag vX.Y.Z`
