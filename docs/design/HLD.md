# High-Level Design (HLD)

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Status | Approved |
| Author | BillyRonks Engineering |
| Last Updated | 2026-02-02 |

---

## 1. Introduction

### 1.1 Purpose

This High-Level Design document describes the overall architecture, key design decisions, and system boundaries for the CoucheStor. It is intended for architects, senior engineers, and technical stakeholders.

### 1.2 Scope

This document covers:
- System architecture and component interactions
- Integration points with external systems
- Key design decisions and rationale
- Non-functional requirements implementation

### 1.3 Audience

- Solution Architects
- Technical Leads
- Senior Engineers
- DevOps Engineers

---

## 2. System Overview

### 2.1 System Context

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Kubernetes Cluster                                 │
│                                                                              │
│  ┌─────────────┐    ┌─────────────────────────┐    ┌─────────────────────┐ │
│  │             │    │                         │    │                     │ │
│  │  Workloads  │───▶│   PersistentVolumes    │◀───│   OpenEBS Mayastor  │ │
│  │             │    │                         │    │                     │ │
│  └─────────────┘    └───────────┬─────────────┘    └──────────┬──────────┘ │
│                                 │                             │            │
│                                 │                             │            │
│                     ┌───────────▼─────────────────────────────▼──────────┐ │
│                     │                                                     │ │
│                     │         CoucheStor                      │ │
│                     │                                                     │ │
│                     │  ┌───────────┐  ┌───────────┐  ┌───────────┐       │ │
│                     │  │  Metrics  │  │Controller │  │ Migrator  │       │ │
│                     │  │  Watcher  │──│           │──│           │       │ │
│                     │  └─────┬─────┘  └───────────┘  └───────────┘       │ │
│                     │        │                                            │ │
│                     └────────┼────────────────────────────────────────────┘ │
│                              │                                              │
│                     ┌────────▼────────┐                                    │
│                     │                 │                                    │
│                     │   Prometheus    │                                    │
│                     │                 │                                    │
│                     └─────────────────┘                                    │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Key Components

| Component | Purpose | Technology |
|-----------|---------|------------|
| StoragePolicy Controller | Reconciles policy CRDs, makes tiering decisions | Rust, kube-rs |
| Metrics Watcher | Collects IOPS metrics from Prometheus | Rust, reqwest |
| Migrator | Executes safe volume migrations | Rust, kube-rs |
| Health Server | Provides health/readiness endpoints | Rust, hyper |
| Metrics Server | Exposes operator metrics | Rust, prometheus |

---

## 3. Architecture Design

### 3.1 Architectural Pattern

The operator implements the **Kubernetes Operator Pattern** combined with an internal **Event-Driven Architecture**:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Operator Architecture                                │
│                                                                              │
│  ┌──────────────────────────────────────────────────────────────────────┐  │
│  │                        Event Sources                                  │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐               │  │
│  │  │  K8s Watch   │  │  Timer       │  │  Manual      │               │  │
│  │  │  (CRD)       │  │  (Requeue)   │  │  (kubectl)   │               │  │
│  │  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘               │  │
│  │         └─────────────────┼─────────────────┘                        │  │
│  │                           ▼                                          │  │
│  │  ┌──────────────────────────────────────────────────────────────┐   │  │
│  │  │                    Work Queue                                 │   │  │
│  │  └────────────────────────┬─────────────────────────────────────┘   │  │
│  │                           ▼                                          │  │
│  │  ┌──────────────────────────────────────────────────────────────┐   │  │
│  │  │                   Reconciler                                  │   │  │
│  │  │  ┌─────────────────────────────────────────────────────────┐ │   │  │
│  │  │  │ 1. Fetch current state                                  │ │   │  │
│  │  │  │ 2. Compare with desired state                           │ │   │  │
│  │  │  │ 3. Take corrective action                               │ │   │  │
│  │  │  │ 4. Update status                                        │ │   │  │
│  │  │  │ 5. Requeue for next iteration                           │ │   │  │
│  │  │  └─────────────────────────────────────────────────────────┘ │   │  │
│  │  └──────────────────────────────────────────────────────────────┘   │  │
│  └──────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 3.2 Component Interaction Model

```
┌────────────────────────────────────────────────────────────────────────────┐
│                      Component Interaction Flow                             │
│                                                                             │
│   ┌─────────────┐                                                          │
│   │ StoragePolicy│                                                          │
│   │    CRD      │                                                          │
│   └──────┬──────┘                                                          │
│          │ Watch                                                           │
│          ▼                                                                 │
│   ┌──────────────┐     Query Heat     ┌──────────────┐                    │
│   │              │◀───────────────────│              │                    │
│   │  Controller  │                    │   Metrics    │                    │
│   │              │                    │   Watcher    │                    │
│   └──────┬───────┘                    └──────┬───────┘                    │
│          │                                   │                             │
│          │ Migration                         │ PromQL                      │
│          │ Request                           │                             │
│          ▼                                   ▼                             │
│   ┌──────────────┐                    ┌──────────────┐                    │
│   │              │                    │              │                    │
│   │   Migrator   │                    │  Prometheus  │                    │
│   │              │                    │              │                    │
│   └──────┬───────┘                    └──────────────┘                    │
│          │                                                                 │
│          │ K8s API                                                         │
│          ▼                                                                 │
│   ┌──────────────┐                                                        │
│   │              │                                                        │
│   │  Mayastor    │                                                        │
│   │  (Volumes)   │                                                        │
│   │              │                                                        │
│   └──────────────┘                                                        │
│                                                                             │
└────────────────────────────────────────────────────────────────────────────┘
```

---

## 4. Design Decisions

### 4.1 Language Choice: Rust

**Decision**: Implement the operator in Rust

**Rationale**:
- Memory safety without garbage collection
- High performance with low resource usage
- Strong type system catches errors at compile time
- Excellent async/await support with Tokio
- Mature Kubernetes ecosystem with kube-rs

**Trade-offs**:
- Steeper learning curve than Go
- Smaller talent pool than Go for K8s operators
- Longer compile times

### 4.2 CRD-Based Configuration

**Decision**: Use Custom Resource Definitions for policy configuration

**Rationale**:
- Native Kubernetes experience
- GitOps-friendly (declarative YAML)
- Built-in validation via OpenAPI schema
- Enables kubectl integration
- Supports namespacing and RBAC

**Alternatives Considered**:
- ConfigMaps: Less structured, no built-in validation
- Command-line flags: Not suitable for per-policy config
- External config store: Adds operational complexity

### 4.3 Prometheus for Metrics Source

**Decision**: Use Prometheus as the sole metrics source

**Rationale**:
- De facto standard for Kubernetes monitoring
- Mayastor already exposes Prometheus metrics
- Powerful query language (PromQL)
- Time-series data supports averaging

**Trade-offs**:
- Hard dependency on Prometheus
- No support for other monitoring systems

### 4.4 4-Phase Migration Model

**Decision**: Implement migrations as a 4-phase process (Analyze → Scale Up → Wait Sync → Scale Down)

**Rationale**:
- **Data safety**: Old replica is preserved until new replica is verified synced
- **Reversibility**: Can abort at any phase without data loss
- **Observability**: Each phase is logged and tracked
- **Timeout protection**: Any phase can time out without causing data loss

**Alternatives Considered**:
- Direct move: Higher risk of data loss
- Two-phase (add then remove): No sync verification

### 4.5 Polling for Sync Status

**Decision**: Poll for replica sync status rather than using watches

**Rationale**:
- Simpler implementation
- Configurable poll interval for tuning
- Works with existing Mayastor API
- Timeout handling is straightforward

**Trade-offs**:
- Higher API load than watch-based approach
- Delayed detection of sync completion

---

## 5. Integration Architecture

### 5.1 Kubernetes API Integration

```
┌─────────────────────────────────────────────────────────────────┐
│                  Kubernetes API Usage                            │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  API Groups Used:                                               │
│  ├── storage.billyronks.io/v1 (StoragePolicy CRD)              │
│  ├── openebs.io/v1 (MayastorVolume, DiskPool)                  │
│  └── core/v1 (PersistentVolume)                                │
│                                                                  │
│  Operations:                                                    │
│  ├── Watch: StoragePolicy (event-driven reconciliation)        │
│  ├── List: PersistentVolumes (per reconciliation)              │
│  ├── Get: MayastorVolume, DiskPool                             │
│  ├── Patch: MayastorVolume (spec), StoragePolicy (status)      │
│  └── Update: PV annotations                                    │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### 5.2 Prometheus Integration

```
┌─────────────────────────────────────────────────────────────────┐
│                  Prometheus Integration                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Query Types:                                                   │
│  ├── Instant query: /api/v1/query                              │
│  └── Range query: /api/v1/query_range (future)                 │
│                                                                  │
│  Metrics Queried:                                               │
│  ├── openebs_volume_iops (primary)                             │
│  ├── mayastor_volume_iops (fallback)                           │
│  └── mayastor_volume_read_ops (fallback)                       │
│                                                                  │
│  Health Check:                                                  │
│  └── GET /-/healthy                                            │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## 6. Deployment Architecture

### 6.1 Deployment Model

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        Deployment Topology                                   │
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                    Kubernetes Namespace: kube-system                 │   │
│  │                                                                       │   │
│  │  ┌─────────────────────────────────────────────────────────────────┐ │   │
│  │  │                Deployment: couchestor                │ │   │
│  │  │                                                                   │ │   │
│  │  │  Replicas: 1 (single leader)                                     │ │   │
│  │  │  Strategy: Recreate (not RollingUpdate)                          │ │   │
│  │  │                                                                   │ │   │
│  │  │  ┌───────────────────────────────────────────────────────────┐  │ │   │
│  │  │  │            Pod: couchestor-xxxxx              │  │ │   │
│  │  │  │                                                            │  │ │   │
│  │  │  │  Container: operator                                       │  │ │   │
│  │  │  │  ├── Image: couchestor:v1.0.0                 │  │ │   │
│  │  │  │  ├── Resources: 100m CPU, 256Mi memory                    │  │ │   │
│  │  │  │  ├── Ports: 8080 (metrics), 8081 (health)                 │  │ │   │
│  │  │  │  └── Probes: liveness, readiness                          │  │ │   │
│  │  │  │                                                            │  │ │   │
│  │  │  └───────────────────────────────────────────────────────────┘  │ │   │
│  │  │                                                                   │ │   │
│  │  └─────────────────────────────────────────────────────────────────┘ │   │
│  │                                                                       │   │
│  │  ┌────────────────────┐  ┌────────────────────────────────────────┐ │   │
│  │  │ Service: metrics   │  │ ServiceAccount: couchestor │ │   │
│  │  │ Port: 8080         │  │ ClusterRole: cluster-admin equivalent  │ │   │
│  │  └────────────────────┘  └────────────────────────────────────────┘ │   │
│  │                                                                       │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 6.2 RBAC Design

```yaml
# ClusterRole
rules:
  # StoragePolicy management
  - apiGroups: ["storage.billyronks.io"]
    resources: ["storagepolicies", "storagepolicies/status"]
    verbs: ["get", "list", "watch", "update", "patch"]

  # Mayastor resources
  - apiGroups: ["openebs.io"]
    resources: ["diskpools", "mayastorvolumes"]
    verbs: ["get", "list", "watch", "update", "patch"]

  # PersistentVolumes
  - apiGroups: [""]
    resources: ["persistentvolumes"]
    verbs: ["get", "list", "watch", "update", "patch"]

  # Events (for recording)
  - apiGroups: [""]
    resources: ["events"]
    verbs: ["create", "patch"]
```

---

## 7. Non-Functional Requirements

### 7.1 Performance

| Metric | Target | Implementation |
|--------|--------|----------------|
| Reconciliation time | < 30s | Efficient list/filter operations |
| Memory usage | < 256 MB | Rust memory efficiency, bounded caches |
| CPU (idle) | < 50m | Event-driven, no busy loops |
| Startup time | < 10s | Minimal initialization |

### 7.2 Reliability

| Requirement | Implementation |
|-------------|----------------|
| Crash recovery | Kubernetes restarts, state in etcd |
| Data safety | 4-phase migration, no unsafe state transitions |
| Graceful shutdown | Signal handling, migration completion |
| Error handling | Requeue on error, exponential backoff |

### 7.3 Scalability

| Dimension | Limit | Rationale |
|-----------|-------|-----------|
| Volumes per cluster | 1000+ | Paginated list, efficient reconciliation |
| Policies per cluster | 100+ | Independent reconciliation |
| Concurrent migrations | Configurable | Semaphore-based limiting |

### 7.4 Observability

| Aspect | Implementation |
|--------|----------------|
| Logging | Structured logs, configurable level |
| Metrics | Prometheus exposition |
| Tracing | Span-based instrumentation |
| Status | CRD status subresource |

---

## 8. Security Design

### 8.1 Authentication & Authorization

- Uses Kubernetes ServiceAccount for API access
- RBAC permissions scoped to required resources
- No external authentication required

### 8.2 Network Security

- No ingress required
- Internal cluster communication only
- Egress: Kubernetes API, Prometheus

### 8.3 Data Security

- No persistent storage required
- No secrets management needed
- Logs do not contain sensitive data

---

## 9. Technology Stack

| Layer | Technology | Version |
|-------|------------|---------|
| Language | Rust | 2021 Edition |
| Async Runtime | Tokio | 1.36+ |
| Kubernetes Client | kube-rs | 0.88 |
| HTTP Client | reqwest | 0.11 |
| Serialization | serde | 1.0 |
| Metrics | prometheus | 0.13 |
| HTTP Server | hyper | 0.14 |
| CLI | clap | 4.5 |
| Error Handling | thiserror | 1.0 |
| Caching | dashmap | 5.5 |

---

## 10. Future Considerations

### 10.1 Planned Enhancements

| Enhancement | Impact | Priority |
|-------------|--------|----------|
| Predictive tiering | Use ML for proactive migrations | Medium |
| Multi-cluster | Federated policies | Low |
| Cost optimization | Factor in storage costs | Medium |
| Custom metrics | Support non-IOPS metrics | Medium |

### 10.2 Known Limitations

| Limitation | Workaround |
|------------|------------|
| Single metrics source | Configure Prometheus federation |
| No HA (single replica) | Use PodDisruptionBudget |
| Mayastor-specific | Fork for other storage backends |
