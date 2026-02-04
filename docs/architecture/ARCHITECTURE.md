# CoucheStor - Architecture Overview

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Status | Approved |
| Author | BillyRonks Engineering |
| Last Updated | 2026-02-02 |

## 1. Executive Summary

The CoucheStor is a Kubernetes-native operator that provides intelligent, automated storage tiering for OpenEBS Mayastor deployments. It continuously monitors volume performance metrics and automatically migrates data between high-performance NVMe and cost-effective SATA storage tiers based on configurable policies.

## 2. Architecture Principles

### 2.1 Design Philosophy

The operator follows the **"Eyes, Brain, Hands"** architectural pattern:

```
┌─────────────────────────────────────────────────────────────────┐
│                     CoucheStor                       │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐       │
│  │   Metrics    │───▶│  Controller  │───▶│   Migrator   │       │
│  │   Watcher    │    │    (Brain)   │    │   (Hands)    │       │
│  │   (Eyes)     │    │              │    │              │       │
│  └──────────────┘    └──────────────┘    └──────────────┘       │
└─────────────────────────────────────────────────────────────────┘
```

- **Eyes (MetricsWatcher)**: Observes volume performance by querying Prometheus
- **Brain (Controller)**: Makes intelligent tiering decisions based on policies
- **Hands (Migrator)**: Executes safe volume migrations with data protection

### 2.2 Core Principles

| Principle | Description |
|-----------|-------------|
| **Data Safety First** | Old replicas are never removed until new replicas are fully synced |
| **Kubernetes Native** | Uses CRDs, controllers, and standard Kubernetes patterns |
| **Observable** | Comprehensive metrics, logging, and status reporting |
| **Configurable** | Policy-driven with sensible defaults |
| **Non-Invasive** | Works alongside existing Mayastor deployments |

## 3. System Architecture

### 3.1 High-Level Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           Kubernetes Cluster                              │
│  ┌─────────────────────────────────────────────────────────────────────┐ │
│  │                        Control Plane                                  │ │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────────┐  │ │
│  │  │  API Server │  │    etcd     │  │   StoragePolicy CRD         │  │ │
│  │  └──────┬──────┘  └─────────────┘  │   (storage.billyronks.io)   │  │ │
│  │         │                          └─────────────────────────────┘  │ │
│  └─────────┼───────────────────────────────────────────────────────────┘ │
│            │                                                              │
│  ┌─────────▼───────────────────────────────────────────────────────────┐ │
│  │                    CoucheStor                            │ │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                  │ │
│  │  │   Metrics   │  │ StoragePolicy│  │  Migration  │                  │ │
│  │  │   Watcher   │  │  Controller  │  │   Engine    │                  │ │
│  │  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘                  │ │
│  │         │                │                │                          │ │
│  │  ┌──────▼──────┐  ┌──────▼──────┐  ┌──────▼──────┐                  │ │
│  │  │  Prometheus │  │  StoragePolicy│ │ MayastorVolume│                │ │
│  │  │   Client    │  │   Reconciler │  │    Client   │                  │ │
│  │  └─────────────┘  └─────────────┘  └─────────────┘                  │ │
│  └─────────────────────────────────────────────────────────────────────┘ │
│                                                                          │
│  ┌─────────────────────────────────────────────────────────────────────┐ │
│  │                         Data Plane                                   │ │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                  │ │
│  │  │   NVMe      │  │   SATA      │  │  Prometheus │                  │ │
│  │  │   Pools     │  │   Pools     │  │   Server    │                  │ │
│  │  │  (Hot Tier) │  │ (Cold Tier) │  │             │                  │ │
│  │  └─────────────┘  └─────────────┘  └─────────────┘                  │ │
│  └─────────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────────┘
```

### 3.2 Component Interactions

```
┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐
│Prometheus│    │ Metrics  │    │Controller│    │ Migrator │
│  Server  │    │ Watcher  │    │          │    │          │
└────┬─────┘    └────┬─────┘    └────┬─────┘    └────┬─────┘
     │               │               │               │
     │◀──PromQL─────▶│               │               │
     │   Queries     │               │               │
     │               │               │               │
     │               │──HeatScore──▶│               │
     │               │               │               │
     │               │               │──Policy──────▶│
     │               │               │  Decision     │
     │               │               │               │
     │               │               │◀──Migration──│
     │               │               │   Result      │
     │               │               │               │
```

## 4. Component Architecture

### 4.1 Metrics Watcher (Eyes)

**Purpose**: Collect and analyze volume performance metrics

**Responsibilities**:
- Query Prometheus for IOPS metrics
- Calculate time-weighted heat scores
- Cache results to reduce API load
- Provide fallback metric sources

**Key Features**:
- Configurable query timeout (default: 30s)
- In-memory caching with TTL (default: 30s)
- Support for multiple metric names
- Exponential decay weighting for recent data

```
┌─────────────────────────────────────────┐
│           MetricsWatcher                 │
├─────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐       │
│  │   HTTP      │  │   Cache     │       │
│  │   Client    │  │  (DashMap)  │       │
│  └──────┬──────┘  └──────┬──────┘       │
│         │                │              │
│  ┌──────▼────────────────▼──────┐       │
│  │       HeatScore Calculator    │       │
│  └───────────────────────────────┘       │
└─────────────────────────────────────────┘
```

### 4.2 Controller (Brain)

**Purpose**: Implement the reconciliation loop and make tiering decisions

**Responsibilities**:
- Watch StoragePolicy CRD changes
- List and filter PersistentVolumes
- Evaluate tiering thresholds
- Enforce cooldown periods
- Coordinate migrations

**Key Features**:
- Kubernetes controller-runtime based
- Graceful shutdown support
- Concurrent migration limiting
- Comprehensive status reporting

### 4.3 Migrator (Hands)

**Purpose**: Execute safe volume migrations between storage tiers

**Responsibilities**:
- Verify source and target pool states
- Add replicas to target pools
- Wait for replica synchronization
- Remove old replicas (only after sync)
- Track migration state

**Key Features**:
- 4-phase migration safety model
- Configurable sync timeout
- Preservation mode option
- Dry-run capability

## 5. Data Flow Architecture

### 5.1 Reconciliation Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                    Reconciliation Cycle                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  1. StoragePolicy Event ──▶ Controller Triggered                │
│                                                                  │
│  2. List PVs ──▶ Filter by StorageClass ──▶ Matching Volumes   │
│                                                                  │
│  3. For Each Volume:                                            │
│     ├── Query HeatScore from MetricsWatcher                     │
│     ├── Compare against thresholds                              │
│     ├── Check cooldown period                                   │
│     └── Trigger migration if needed                             │
│                                                                  │
│  4. Update StoragePolicy Status                                 │
│                                                                  │
│  5. Requeue for next reconciliation (5 minutes)                 │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### 5.2 Migration State Machine

```
         ┌─────────┐
         │  Idle   │
         └────┬────┘
              │ Start
              ▼
         ┌─────────┐
         │Analyzing│──────────────┐
         └────┬────┘              │
              │ Verified          │ Error
              ▼                   │
         ┌─────────┐              │
         │ScalingUp│──────────────┤
         └────┬────┘              │
              │ Replica Added     │
              ▼                   │
         ┌─────────────┐          │
         │WaitingSync  │──────────┤
         └────┬────────┘          │ Timeout
              │ Synced            │
              ▼                   ▼
         ┌───────────┐      ┌─────────┐
         │ScalingDown│      │ Aborted │
         └────┬──────┘      └─────────┘
              │ Removed           │
              ▼                   │
         ┌─────────┐              │
         │Completed│              │
         └─────────┘              │
              │                   │
              └───────────────────┘
                  Data Safe
```

## 6. Deployment Architecture

### 6.1 Kubernetes Resources

| Resource Type | Name | Purpose |
|--------------|------|---------|
| Deployment | couchestor | Main operator workload |
| ServiceAccount | couchestor | RBAC identity |
| ClusterRole | couchestor | API permissions |
| ClusterRoleBinding | couchestor | Binds role to SA |
| CRD | storagepolicies.storage.billyronks.io | Policy definition |
| Service | couchestor-metrics | Metrics endpoint |

### 6.2 High Availability

```
┌─────────────────────────────────────────────────────────────────┐
│                    HA Deployment (Recommended)                   │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌─────────────────┐        ┌─────────────────┐                 │
│  │    Node A       │        │    Node B       │                 │
│  │  ┌───────────┐  │        │  ┌───────────┐  │                 │
│  │  │ Operator  │  │◀──────▶│  │ Operator  │  │                 │
│  │  │ (Active)  │  │ Leader │  │ (Standby) │  │                 │
│  │  └───────────┘  │ Election│  └───────────┘  │                 │
│  └─────────────────┘        └─────────────────┘                 │
│                                                                  │
│  Note: Only one instance is active at a time                    │
│  (leader election via Kubernetes coordination)                  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

## 7. Integration Points

### 7.1 External Systems

| System | Integration Method | Purpose |
|--------|-------------------|---------|
| Prometheus | HTTP/PromQL | Metrics collection |
| Kubernetes API | client-go/kube-rs | Resource management |
| OpenEBS Mayastor | CRD APIs | Volume operations |
| Monitoring Stack | /metrics endpoint | Operator observability |

### 7.2 API Endpoints

| Endpoint | Port | Purpose |
|----------|------|---------|
| /metrics | 8080 | Prometheus metrics |
| /healthz | 8081 | Liveness probe |
| /readyz | 8081 | Readiness probe |

## 8. Security Architecture

### 8.1 RBAC Permissions

```yaml
# Required API permissions
rules:
  - apiGroups: ["storage.billyronks.io"]
    resources: ["storagepolicies", "storagepolicies/status"]
    verbs: ["get", "list", "watch", "update", "patch"]
  - apiGroups: ["openebs.io"]
    resources: ["diskpools", "mayastorvolumes"]
    verbs: ["get", "list", "watch", "update", "patch"]
  - apiGroups: [""]
    resources: ["persistentvolumes"]
    verbs: ["get", "list", "watch"]
```

### 8.2 Network Security

- Internal cluster communication only
- No ingress required
- Egress to Prometheus server (configurable)

## 9. Monitoring Architecture

### 9.1 Exposed Metrics

| Metric | Type | Description |
|--------|------|-------------|
| storage_operator_reconcile_total | Counter | Total reconciliations |
| storage_operator_migrations_total | Counter | Total migrations by status |
| storage_operator_active_migrations | Gauge | Current active migrations |

### 9.2 Logging

- Structured JSON logging (optional)
- Configurable log levels (trace/debug/info/warn/error)
- Request tracing with span IDs

## 10. Technology Stack

| Layer | Technology | Version |
|-------|------------|---------|
| Language | Rust | 2021 Edition |
| Runtime | Tokio | 1.36+ |
| Kubernetes Client | kube-rs | 0.88 |
| HTTP Client | reqwest | 0.11 |
| Metrics | prometheus | 0.13 |
| Serialization | serde | 1.0 |
| CLI | clap | 4.5 |

## 11. Appendix

### 11.1 Glossary

| Term | Definition |
|------|------------|
| Heat Score | Numeric value representing volume activity level |
| Hot Tier | High-performance storage (NVMe) |
| Cold Tier | Cost-effective storage (SATA) |
| Watermark | IOPS threshold for tiering decisions |
| Cooldown | Minimum time between migrations |

### 11.2 References

- [Kubernetes Operator Pattern](https://kubernetes.io/docs/concepts/extend-kubernetes/operator/)
- [OpenEBS Mayastor Documentation](https://mayastor.gitbook.io/)
- [kube-rs Documentation](https://kube.rs/)
