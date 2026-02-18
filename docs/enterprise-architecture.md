# Enterprise Architecture Document — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Enterprise Context

CoucheStor Community Edition fits within a broader enterprise storage infrastructure, integrating with Kubernetes ecosystems, monitoring stacks, and organizational IT governance frameworks.

## 2. Business Architecture

### 2.1 Business Capability Map
```
Enterprise Storage Management
├── Data Lifecycle Management
│   ├── Automated Tiering .............. [CoucheStor CE]
│   ├── Data Retention Policies ........ [External/Manual]
│   └── Archival Management ............ [Cold Tier + EC]
├── Cost Optimization
│   ├── Storage Efficiency ............. [Erasure Coding]
│   ├── Capacity Planning .............. [Prometheus Metrics]
│   └── Tier Cost Analysis ............. [Monitoring Stack]
├── Data Protection
│   ├── Fault Tolerance ................ [Reed-Solomon EC]
│   ├── Migration Safety ............... [4-Step Process]
│   └── Backup/Recovery ................ [External]
├── Observability
│   ├── Metrics Collection ............. [Prometheus]
│   ├── Health Monitoring .............. [K8s Probes]
│   └── Alerting ....................... [AlertManager]
└── Platform Operations
    ├── Kubernetes Management .......... [K8s API]
    ├── Storage Provisioning ........... [OpenEBS Mayastor]
    └── Configuration Management ....... [CRDs]
```

### 2.2 Organizational Integration Points

| Integration Point | Interface | Protocol |
|-------------------|-----------|----------|
| Monitoring Team | Prometheus metrics (:8080/metrics) | HTTP/Prometheus exposition |
| Platform Team | Kubernetes CRDs | K8s API (HTTPS) |
| Storage Team | Mayastor CRDs (DiskPool, MayastorVolume) | K8s API (HTTPS) |
| SRE Team | Health probes (:8081) | HTTP |
| Security Team | RBAC ClusterRole, SecurityContext | K8s RBAC |

## 3. Information Architecture

### 3.1 Data Classification

| Data Type | Classification | Storage | Retention |
|-----------|---------------|---------|-----------|
| StoragePolicy CRDs | Operational | etcd (K8s) | Cluster lifetime |
| ErasureCodingPolicy CRDs | Operational | etcd (K8s) | Cluster lifetime |
| ECStripe CRDs | Critical | etcd (K8s) | Volume lifetime |
| Volume data (hot) | Business-critical | NVMe pools | Application-defined |
| Volume data (warm) | Business-critical | SAS/SATA pools | Application-defined |
| Volume data (cold/EC) | Business-critical | HDD pools (EC shards) | Application-defined |
| Prometheus metrics | Operational | Prometheus TSDB | Configurable (default 15d) |
| Operator logs | Operational | stdout/stderr | Log aggregator-defined |
| Migration history | Audit | StoragePolicy status | Last 50 entries |

### 3.2 Data Flow Diagram

```
┌──────────────────┐     Prometheus Query      ┌──────────────────┐
│   Prometheus      │◀────────────────────────│   MetricsWatcher  │
│   (TSDB)          │  rate(iops[1h])         │   (Eyes)          │
└──────────────────┘                           └────────┬─────────┘
                                                         │
                                                         ▼
┌──────────────────┐     Reconciliation        ┌──────────────────┐
│   Kubernetes      │◀───────────────────────│   Controller      │
│   API Server      │  CRUD on CRDs           │   (Brain)         │
│   (etcd)          │                          └────────┬─────────┘
└──────────────────┘                                     │
        ▲                                                │
        │                                                ▼
        │              Mayastor API             ┌──────────────────┐
        └──────────────────────────────────────│   Migrator        │
                Add/Remove Replicas             │   (Hands)         │
                                                └──────────────────┘
```

## 4. Technology Architecture

### 4.1 Infrastructure Stack

```
┌─────────────────────────────────────────────────────────────────┐
│                        Applications Layer                        │
│  Stateful workloads (databases, message queues, ML pipelines)   │
├─────────────────────────────────────────────────────────────────┤
│                        Platform Layer                            │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐       │
│  │CoucheStor│  │OpenEBS   │  │Prometheus│  │Kubernetes│       │
│  │Operator  │  │Mayastor  │  │Stack     │  │1.28+     │       │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘       │
├─────────────────────────────────────────────────────────────────┤
│                      Infrastructure Layer                        │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐       │
│  │NVMe SSDs │  │SAS SSDs  │  │SATA HDDs │  │Network   │       │
│  │(Hot Tier)│  │(Warm)    │  │(Cold)    │  │(10GbE+)  │       │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘       │
└─────────────────────────────────────────────────────────────────┘
```

### 4.2 Recommended Supporting Infrastructure

| Component | Purpose | Recommended Technology |
|-----------|---------|----------------------|
| Metadata Store | CRD persistence | Kubernetes etcd (built-in) |
| Cache Layer | Performance acceleration | DragonflyDB (Redis-compatible) |
| Event Bus | Domain event streaming | Redpanda (Kafka-compatible) |
| Log Aggregation | Centralized logging | Quickwit (full-text search) |
| Analytics | Storage analytics | ClickHouse (columnar OLAP) |
| Messaging | Internal messaging | NATS (lightweight pub/sub) |
| Monitoring | Metrics, alerting | Prometheus + Grafana |
| Tracing | Distributed tracing | OpenTelemetry Collector |

### 4.3 Network Architecture

| Source | Destination | Port | Protocol | Purpose |
|--------|-------------|------|----------|---------|
| CoucheStor | K8s API Server | 443 | HTTPS | CRD management |
| CoucheStor | Prometheus | 9090 | HTTP | Metrics queries |
| Prometheus | CoucheStor | 8080 | HTTP | Scrape metrics |
| K8s Kubelet | CoucheStor | 8081 | HTTP | Health probes |
| Mayastor | DiskPools | - | NVMe/iSCSI | Data I/O |

## 5. Application Architecture

### 5.1 Deployment Topology

**Single Cluster Deployment**:
```
Kubernetes Cluster
├── couchestor-system namespace
│   └── couchestor-operator (Deployment, 1 replica)
├── mayastor namespace
│   ├── mayastor-io-engine (DaemonSet)
│   ├── mayastor-api-rest (Deployment)
│   └── mayastor-csi (DaemonSet)
├── monitoring namespace
│   ├── prometheus (StatefulSet)
│   ├── alertmanager (Deployment)
│   └── grafana (Deployment)
└── application namespaces
    └── workloads using Mayastor PVCs
```

**Multi-Cluster Deployment** (Enterprise upgrade path):
```
Cluster A (Primary)              Cluster B (DR)
├── CoucheStor CE               ├── CoucheStor EE
├── Mayastor                    ├── Mayastor
├── Hot/Warm/Cold pools         ├── Cold pools
└── Active workloads            └── Standby replicas
```

### 5.2 Scalability Model

| Dimension | CE Limits | Scaling Strategy |
|-----------|-----------|-----------------|
| Volumes managed | 1000+ | Controller watch with field selectors |
| Concurrent migrations | Configurable (default 2) | Semaphore-based limiting |
| EC stripes | 100K+ per cluster | ECStripe CRDs in etcd |
| Cache capacity | L1: 50GB, L2: 500GB, L3: 10TB+ | Tier-based overflow |
| Prometheus queries | Batched, cached (30s TTL) | Query deduplication |

## 6. Security Architecture

### 6.1 RBAC Model
```yaml
ClusterRole: couchestor-operator
  storage.billyronks.io:
    - storagepolicies: get, list, watch, update, patch
    - erasurecodingpolicies: get, list, watch, update, patch
  openebs.io:
    - diskpools: get, list, watch, update, patch
    - mayastorvolumes: get, list, watch, update, patch
  core:
    - persistentvolumes: get, list, watch
    - persistentvolumeclaims: get, list, watch
    - events: create, patch
  storage.k8s.io:
    - storageclasses: get, list, watch
  coordination.k8s.io:
    - leases: get, create, update
```

### 6.2 Container Security Profile
- Image: Distroless (no shell, no package manager)
- User: nonroot (UID 65534)
- Filesystem: Read-only
- Capabilities: All dropped
- Privilege escalation: Disabled
- Seccomp: Runtime default

### 6.3 Data Security
- Volume data: Protected by Mayastor's encryption capabilities
- EC shards: Distributed across pools (no single pool holds complete data)
- CRD data: Protected by Kubernetes etcd encryption at rest
- Metrics data: No PII, operational metrics only
- Operator secrets: None stored (uses ServiceAccount token)

## 7. Integration Architecture

### 7.1 Integration Patterns

| Pattern | Implementation | Use Case |
|---------|---------------|----------|
| Observer | K8s watch streams | CRD change detection |
| Polling | Prometheus queries | Metrics collection |
| Command | K8s API calls | Volume migrations |
| Event Sourcing | DomainEvent types | Migration history |
| Circuit Breaker | (Planned) | External service resilience |

### 7.2 API Contract Versions

| API | Version | Stability |
|-----|---------|-----------|
| StoragePolicy | v1 | Stable |
| ErasureCodingPolicy | v1 | Stable |
| ECStripe | v1 | Stable |
| Metrics endpoint | Prometheus v2 | Stable |
| Health endpoints | HTTP/1.1 | Stable |
| DiskPool (Mayastor) | v1beta2 | Beta |
| MayastorVolume | v1alpha1 | Alpha |

## 8. Governance and Compliance

### 8.1 CE Compliance Scope
- Apache 2.0 license: Permissive, allows commercial use
- No PII handling: Operator manages infrastructure metadata only
- Audit trail: Migration history stored in CRD status (last 50 entries)
- For SOC2/HIPAA audit logging: Upgrade to Enterprise Edition

### 8.2 Operational Governance
- Change management: StoragePolicy CRDs are version-controlled (GitOps compatible)
- Capacity planning: Prometheus metrics for pool utilization
- Incident response: Health probes + structured logging + migration history
- Disaster recovery: EC provides m-shard fault tolerance; backups are external

## 9. Migration Strategy (CE to EE)

| Step | Action | Impact |
|------|--------|--------|
| 1 | Deploy EE binary alongside CE | No downtime |
| 2 | Apply EE CRDs (TenantPolicy, ReplicationPolicy, AuditPolicy) | Additive only |
| 3 | Configure EE features (multi-tenancy, replication) | New features enabled |
| 4 | Remove CE deployment | Clean switch |
| 5 | Verify all StoragePolicies remain functional | Zero data migration |

CE configurations remain 100% compatible with EE.
