# CoucheStor Editions Comparison

CoucheStor is available in two editions: **Community Edition (CE)** and **Enterprise Edition (EE)**.

## Quick Comparison

| Metric | Community (CE) | Enterprise (EE) |
|--------|----------------|-----------------|
| License | Apache 2.0 | Proprietary |
| Rust Files | 61 | 74 |
| Lines of Code | ~33,000 | ~38,000 |
| Integration Tests | 34 | 60 |

## Feature Matrix

| Feature | Community (CE) | Enterprise (EE) |
|---------|:--------------:|:---------------:|
| **Core Storage** | | |
| Tiered Storage (Hot/Warm/Cold) | ✓ | ✓ |
| Erasure Coding (4+2 Reed-Solomon) | ✓ | ✓ |
| Kubernetes CRDs | ✓ | ✓ |
| Hardware Discovery (NVMe/SAS/SATA) | ✓ | ✓ |
| **Caching** | | |
| L1 Cache (RAM, 1024-way sharded) | ✓ | ✓ |
| L2 Cache (NVMe, memory-mapped) | ✓ | ✓ |
| L3 Cache (Cold storage backend) | ✓ | ✓ |
| Async Prefetch / Cache Warming | - | ✓ |
| **Compression** | | |
| LZ4 (fast) | ✓ | ✓ |
| Zstd (balanced) | - | ✓ |
| Snappy (very fast) | - | ✓ |
| **Multi-Tenancy** | | |
| Tenant Management | - | ✓ |
| Per-Tenant Quotas | - | ✓ |
| Rate Limiting (Token Bucket) | - | ✓ |
| Tenant Tiers (Free/Pro/Enterprise) | - | ✓ |
| **Replication** | | |
| Multi-Region Active-Active | - | ✓ |
| Circuit Breaker (Failure Isolation) | - | ✓ |
| Conflict Resolution (Vector Clock) | - | ✓ |
| Sync/Async/SemiSync Modes | - | ✓ |
| **Compliance & Audit** | | |
| SOC2/HIPAA Audit Logging | - | ✓ |
| Audit Event Streaming | - | ✓ |
| **Observability** | | |
| Prometheus Metrics | ✓ | ✓ |
| Health Checks (Liveness/Readiness) | ✓ | ✓ |
| Performance Histograms | ✓ | ✓ |

## Module Differences

### Files Only in Enterprise Edition

```
src/audit.rs                      # SOC2/HIPAA audit logging
src/rustfs/cache/prefetch.rs      # Async cache warming
src/rustfs/tenancy/               # Multi-tenant management
  ├── manager.rs                  # 1024-way sharded tenant registry
  ├── tenant.rs                   # Tenant types and status
  ├── quota.rs                    # Quota enforcement
  └── rate_limiter.rs             # Token bucket rate limiting
src/rustfs/replication/           # Multi-region replication
  ├── manager.rs                  # Replication orchestrator
  ├── event.rs                    # Replication events
  ├── connector.rs                # Region communication
  ├── circuit_breaker.rs          # Failure isolation
  └── conflict.rs                 # Vector clock resolution
```

### Compression Algorithms

**Community Edition:**
```rust
pub enum CompressionAlgorithm {
    None,
    Lz4,   // Fast compression
}
```

**Enterprise Edition:**
```rust
pub enum CompressionAlgorithm {
    None,
    Lz4,     // Fast compression
    Zstd,    // Balanced compression (better ratio)
    Snappy,  // Very fast (lower ratio)
}
```

## Performance Targets

Both editions share the same performance targets for core functionality:

| Operation | Latency | Throughput |
|-----------|---------|------------|
| L1 Cache Read | < 1μs | 2M ops/sec |
| L1 Cache Write | < 5μs | 500K ops/sec |
| L2 Cache Read | < 100μs | 500K ops/sec |
| L3 Cache Read | < 10ms | 10K ops/sec |

**Enterprise Edition Additional:**

| Operation | Latency | Throughput |
|-----------|---------|------------|
| Quota Lookup | < 1μs | 100K+ ops/sec |
| Replication Event | async | 10K+ ops/sec |

## Which Edition Should I Use?

### Choose Community Edition if:
- You need basic tiered storage with erasure coding
- Single-tenant deployment is sufficient
- LZ4 compression meets your needs
- You don't require audit logging for compliance

### Choose Enterprise Edition if:
- You need multi-tenant isolation with quotas
- Multi-region replication is required
- You need SOC2/HIPAA compliance audit trails
- You want additional compression options (Zstd, Snappy)
- Cache warming/prefetch improves your workload

## Repository Links

- **Community Edition**: https://github.com/abiolaogu/couchestor-ce (Public)
- **Enterprise Edition**: https://github.com/abiolaogu/couchestor-ee (Private)

## Upgrade Path

Upgrading from CE to EE requires no data migration. Simply deploy the EE binary and configure the additional features as needed. All CE configurations remain compatible.
