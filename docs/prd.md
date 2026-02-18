# Product Requirements Document — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Product Overview

### 1.1 Product Vision
CoucheStor Community Edition is a Kubernetes-native intelligent tiered storage operator that automatically migrates volumes between Hot (NVMe), Warm (SAS/SATA SSD), and Cold (HDD/archival) tiers based on real-time IOPS metrics. It provides Reed-Solomon erasure coding for storage-efficient cold tier, reducing storage overhead from 200% (3-way replication) to 50% (4+2 EC).

### 1.2 Target Users
- **Infrastructure Engineers**: Managing Kubernetes storage infrastructure with OpenEBS Mayastor
- **Platform Teams**: Building self-service storage platforms with automated tiering
- **DevOps Engineers**: Deploying and operating storage-intensive applications
- **Storage Administrators**: Optimizing storage costs through intelligent data placement

### 1.3 Problem Statement
Modern Kubernetes workloads generate data with varying access patterns. Hot data (frequently accessed) requires NVMe performance, while cold data (rarely accessed) wastes expensive NVMe capacity. Manual data migration is error-prone, time-consuming, and does not scale. CoucheStor automates this process with policy-driven, metrics-based decisions.

## 2. Functional Requirements

### 2.1 Tiered Storage Management (FR-100)

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-101 | System SHALL support three storage tiers: Hot, Warm, Cold | P0 |
| FR-102 | System SHALL automatically migrate volumes between tiers based on IOPS thresholds | P0 |
| FR-103 | System SHALL query Prometheus for volume IOPS metrics over configurable sampling windows | P0 |
| FR-104 | System SHALL enforce cooldown periods between consecutive migrations of the same volume | P0 |
| FR-105 | System SHALL limit concurrent migrations to a configurable maximum | P0 |
| FR-106 | System SHALL support dry-run mode for policy testing | P1 |
| FR-107 | System SHALL support preservation mode that never removes old replicas | P1 |
| FR-108 | System SHALL track migration history (last 50 entries) per StoragePolicy | P1 |

### 2.2 Erasure Coding (FR-200)

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-201 | System SHALL implement 4+2 Reed-Solomon erasure coding for cold tier | P0 |
| FR-202 | System SHALL support configurable data (k) and parity (m) shard counts | P0 |
| FR-203 | System SHALL tolerate up to m shard failures without data loss | P0 |
| FR-204 | System SHALL provide transparent degraded reads when shards are missing | P1 |
| FR-205 | System SHALL automatically reconstruct missing shards in background | P1 |
| FR-206 | System SHALL support background scrubbing for bit rot detection | P2 |
| FR-207 | System SHALL buffer writes in a journal before EC encoding (destaging) | P1 |
| FR-208 | System SHALL support ReedSolomon and LRC algorithms | P1 |

### 2.3 Caching (FR-300)

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-301 | System SHALL implement L1 (RAM) cache with 1024-way sharding | P0 |
| FR-302 | System SHALL implement L2 (NVMe) cache with memory-mapped files | P0 |
| FR-303 | System SHALL implement L3 (Cold storage) async backend | P0 |
| FR-304 | System SHALL support LZ4 compression for cached entries | P0 |
| FR-305 | System SHALL automatically promote/demote entries between tiers | P1 |
| FR-306 | System SHALL provide cache hit/miss ratio metrics | P1 |

### 2.4 Hardware Discovery (FR-400)

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-401 | System SHALL auto-detect NVMe devices via sysfs | P1 |
| FR-402 | System SHALL auto-detect SAS/SATA devices | P1 |
| FR-403 | System SHALL detect ZNS (Zoned Namespace) NVMe devices | P2 |
| FR-404 | System SHALL retrieve SMART data for health monitoring | P2 |

### 2.5 Kubernetes Integration (FR-500)

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-501 | System SHALL define StoragePolicy CRD with API group storage.billyronks.io/v1 | P0 |
| FR-502 | System SHALL define ErasureCodingPolicy CRD | P0 |
| FR-503 | System SHALL define ECStripe CRD for stripe metadata tracking | P0 |
| FR-504 | System SHALL interact with OpenEBS Mayastor CRDs (DiskPool, MayastorVolume) | P0 |
| FR-505 | System SHALL implement controller reconciliation loop for StoragePolicy | P0 |
| FR-506 | System SHALL implement controller reconciliation loop for ErasureCodingPolicy | P0 |

### 2.6 Observability (FR-600)

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-601 | System SHALL expose Prometheus metrics on :8080/metrics | P0 |
| FR-602 | System SHALL provide liveness probe at :8081/healthz | P0 |
| FR-603 | System SHALL provide readiness probe at :8081/readyz | P0 |
| FR-604 | System SHALL emit structured JSON logs when configured | P1 |
| FR-605 | System SHALL track reconciliation counts, migration counts, active migrations, EC stripe totals | P0 |

## 3. Non-Functional Requirements

### 3.1 Performance

| ID | Requirement | Target |
|----|-------------|--------|
| NFR-101 | L1 Cache Read Latency | < 1 microsecond |
| NFR-102 | L1 Cache Write Latency | < 5 microseconds |
| NFR-103 | L2 Cache Read Latency | < 100 microseconds |
| NFR-104 | L3 Cache Read Latency | < 10 milliseconds |
| NFR-105 | L1 Cache Read Throughput | 2M ops/sec |
| NFR-106 | L1 Cache Write Throughput | 500K ops/sec |
| NFR-107 | Binary Size | < 10MB (release, stripped) |
| NFR-108 | Memory Footprint | < 128Mi (base operator) |

### 3.2 Reliability

| ID | Requirement | Target |
|----|-------------|--------|
| NFR-201 | Migration Data Safety | Zero data loss on any failure |
| NFR-202 | EC Fault Tolerance | Survive m shard failures |
| NFR-203 | Operator Availability | Restart within 10s of crash |
| NFR-204 | Migration Timeout | Configurable, default 30 minutes |

### 3.3 Compatibility

| ID | Requirement | Target |
|----|-------------|--------|
| NFR-301 | Rust Version | 1.76+ |
| NFR-302 | Kubernetes Version | 1.28+ |
| NFR-303 | OpenEBS Mayastor | Compatible with v2.x |
| NFR-304 | Prometheus | Compatible with v2.x |

## 4. Feature Exclusions (Enterprise Only)

The following features are NOT included in Community Edition:
- Multi-tenancy (tenant management, quotas, rate limiting)
- Multi-region active-active replication
- SOC2/HIPAA audit logging
- Zstd and Snappy compression algorithms
- Async prefetch / cache warming
- Circuit breaker failure isolation
- Vector clock conflict resolution

## 5. Release Criteria

### 5.1 Quality Gates
- All 34 integration tests passing
- Zero cargo clippy warnings
- Code formatted with cargo fmt
- SPDK mock tests passing with --features mock-spdk
- Binary builds under 10MB (release profile with LTO)

### 5.2 Documentation Gates
- CLAUDE.md accurate and up-to-date
- README.md with quick start guide
- CRD examples in deploy/examples/
- API reference for all public types

## 6. Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| Storage Cost Reduction | 30-50% | EC overhead vs replication |
| Migration Success Rate | > 99% | Prometheus metrics |
| Mean Migration Duration | < 5 minutes | For typical 10GB volumes |
| Operator Restart Time | < 10 seconds | K8s health probes |
| Cache Hit Ratio | > 80% | For repeat access patterns |

## 7. Dependencies

| Dependency | Version | Purpose |
|------------|---------|---------|
| kube | 0.99 | Kubernetes client and controller runtime |
| tokio | 1.36 | Async runtime |
| reed-solomon-erasure | 6.0 | Pure Rust Reed-Solomon codec |
| lz4 | 1.28 | LZ4 compression |
| prometheus | 0.14 | Metrics exposition |
| hyper | 1.5 | HTTP server for metrics/health |
| clap | 4.5 | CLI argument parsing |
| schemars | 0.8 | JSON Schema generation for CRDs |
