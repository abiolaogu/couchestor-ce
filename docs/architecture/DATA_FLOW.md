# Data Flow Documentation

## Document Information

| Field | Value |
|-------|-------|
| Version | 1.0.0 |
| Status | Approved |
| Author | BillyRonks Engineering |
| Last Updated | 2026-02-02 |

## 1. Overview

This document describes the data flows within the CoucheStor, including how data moves between components and external systems.

## 2. Primary Data Flows

### 2.1 Metrics Collection Flow

```
┌─────────────┐         ┌─────────────┐         ┌─────────────┐
│  Prometheus │ ◀─────▶ │   Metrics   │ ──────▶ │  Controller │
│   Server    │  HTTP   │   Watcher   │ HeatScore│            │
└─────────────┘         └─────────────┘         └─────────────┘
       ▲                                               │
       │                                               │
       │    ┌──────────────────────────────────────────┘
       │    │
       │    ▼
┌─────────────────────────────────────────────────────────────────┐
│                      Data Transformation                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Prometheus Response (JSON):                                    │
│  {                                                              │
│    "status": "success",                                         │
│    "data": {                                                    │
│      "resultType": "vector",                                    │
│      "result": [{                                               │
│        "metric": {"volume_id": "pvc-abc123"},                   │
│        "value": [1706867400.123, "5234.56"]                     │
│      }]                                                         │
│    }                                                            │
│  }                                                              │
│                                                                  │
│  ────────────────────── Transforms To ──────────────────────    │
│                                                                  │
│  HeatScore {                                                    │
│    volume_id: "pvc-abc123",                                     │
│    score: 5234.56,                                              │
│    read_iops: 2617.28,                                          │
│    write_iops: 2617.28,                                         │
│    latency_us: None,                                            │
│    sample_count: 1,                                             │
│    calculated_at: "2026-02-02T10:30:00Z",                       │
│    window: Duration(3600s),                                     │
│    source_metric: "openebs_volume_iops"                         │
│  }                                                              │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 Policy Reconciliation Flow

```
┌─────────────────────────────────────────────────────────────────────────┐
│                     Policy Reconciliation Data Flow                      │
└─────────────────────────────────────────────────────────────────────────┘

Step 1: Watch Event
┌─────────────┐         ┌─────────────┐
│  K8s API    │ ──────▶ │  Controller │
│  Server     │ Watch   │  Queue      │
└─────────────┘ Event   └─────────────┘
       │
       │ StoragePolicy CR
       ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ {                                                                        │
│   "apiVersion": "storage.billyronks.io/v1",                             │
│   "kind": "StoragePolicy",                                              │
│   "metadata": { "name": "database-tiering" },                           │
│   "spec": {                                                             │
│     "highWatermarkIOPS": 5000,                                          │
│     "lowWatermarkIOPS": 500,                                            │
│     "storageClassName": "mayastor",                                     │
│     "samplingWindow": "1h"                                              │
│   }                                                                     │
│ }                                                                       │
└─────────────────────────────────────────────────────────────────────────┘

Step 2: List PVs
┌─────────────┐         ┌─────────────┐
│  K8s API    │ ◀─────▶ │  Controller │
│  Server     │  LIST   │             │
└─────────────┘ PVs     └─────────────┘
       │
       │ PersistentVolume List
       ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ items:                                                                   │
│   - metadata:                                                           │
│       name: pvc-abc123                                                  │
│       annotations:                                                      │
│         storage.billyronks.io/last-migration: "2026-02-01T10:00:00Z"   │
│     spec:                                                               │
│       storageClassName: mayastor                                        │
│       csi:                                                              │
│         volumeHandle: "pvc-abc123-uuid"                                 │
└─────────────────────────────────────────────────────────────────────────┘

Step 3: Get Heat Scores
┌─────────────┐         ┌─────────────┐
│  Metrics    │ ◀─────▶ │  Controller │
│  Watcher    │ Query   │             │
└─────────────┘ Scores  └─────────────┘

Step 4: Make Decisions
┌─────────────────────────────────────────────────────────────────────────┐
│                      Decision Matrix                                     │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  Volume: pvc-abc123                                                     │
│  Current Score: 6500 IOPS                                               │
│  High Watermark: 5000 IOPS                                              │
│  Low Watermark: 500 IOPS                                                │
│                                                                          │
│  Decision: MIGRATE TO HOT TIER (NVMe)                                   │
│                                                                          │
│  Checks:                                                                │
│    ✓ Score > High Watermark (6500 > 5000)                              │
│    ✓ Cooldown Period Elapsed (24h since last migration)                │
│    ✓ Migration Semaphore Available                                     │
│    ✓ Not Already Migrating                                             │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘

Step 5: Update Status
┌─────────────┐         ┌─────────────┐
│  K8s API    │ ◀────── │  Controller │
│  Server     │ PATCH   │             │
└─────────────┘ Status  └─────────────┘
       │
       │ StoragePolicy Status
       ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ status:                                                                  │
│   phase: Active                                                         │
│   watchedVolumes: 50                                                    │
│   hotVolumes: 15                                                        │
│   coldVolumes: 30                                                       │
│   activeMigrations: 1                                                   │
│   lastReconcileTime: "2026-02-02T10:30:00Z"                            │
└─────────────────────────────────────────────────────────────────────────┘
```

### 2.3 Migration Execution Flow

```
┌─────────────────────────────────────────────────────────────────────────┐
│                      Migration Data Flow                                 │
└─────────────────────────────────────────────────────────────────────────┘

Phase 1: Analyze
┌─────────────┐         ┌─────────────┐
│  K8s API    │ ◀─────▶ │  Migrator   │
│  Server     │  GET    │             │
└─────────────┘         └─────────────┘
       │
       │ MayastorVolume & DiskPool
       ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ MayastorVolume {                                                         │
│   metadata: { name: "pvc-abc123" },                                     │
│   spec: { numReplicas: 1 },                                             │
│   status: {                                                             │
│     replicas: [{                                                        │
│       pool: "pool-sata-1",                                              │
│       state: "Online",                                                  │
│       syncState: "Synced"                                               │
│     }]                                                                  │
│   }                                                                     │
│ }                                                                       │
│                                                                          │
│ DiskPool {                                                              │
│   metadata: { name: "pool-nvme-1", labels: { tier: "hot" } },           │
│   status: { state: "Online", capacity: "500Gi", used: "200Gi" }         │
│ }                                                                       │
└─────────────────────────────────────────────────────────────────────────┘

Phase 2: Scale Up
┌─────────────┐         ┌─────────────┐
│  K8s API    │ ◀────── │  Migrator   │
│  Server     │ PATCH   │             │
└─────────────┘         └─────────────┘
       │
       │ Patch Volume Spec
       ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ PATCH /apis/openebs.io/v1/namespaces/mayastor/mayastorvolumes/pvc-abc123│
│                                                                          │
│ {                                                                        │
│   "spec": {                                                             │
│     "numReplicas": 2,                                                   │
│     "topology": {                                                       │
│       "pool": {                                                         │
│         "labelled": {                                                   │
│           "inclusion": { "pool": "pool-nvme-1" }                        │
│         }                                                               │
│       }                                                                 │
│     }                                                                   │
│   }                                                                     │
│ }                                                                       │
└─────────────────────────────────────────────────────────────────────────┘

Phase 3: Wait Sync (Polling Loop)
┌─────────────┐         ┌─────────────┐
│  K8s API    │ ◀─────▶ │  Migrator   │
│  Server     │  GET    │   (poll)    │
└─────────────┘ Status  └─────────────┘
       │
       │ Replica Status (every 10s)
       ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ Poll 1: { state: "Unknown", syncState: "Unknown" }      ⏳ Continue     │
│ Poll 2: { state: "Online", syncState: "Syncing" }       ⏳ Continue     │
│ Poll 3: { state: "Online", syncState: "Syncing" }       ⏳ Continue     │
│ Poll N: { state: "Online", syncState: "Synced" }        ✅ Proceed      │
└─────────────────────────────────────────────────────────────────────────┘

Phase 4: Scale Down
┌─────────────┐         ┌─────────────┐
│  K8s API    │ ◀────── │  Migrator   │
│  Server     │ PATCH   │             │
└─────────────┘         └─────────────┘
       │
       │ Reduce Replica Count
       ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ PATCH /apis/openebs.io/v1/namespaces/mayastor/mayastorvolumes/pvc-abc123│
│                                                                          │
│ {                                                                        │
│   "spec": {                                                             │
│     "numReplicas": 1                                                    │
│   }                                                                     │
│ }                                                                       │
└─────────────────────────────────────────────────────────────────────────┘
```

## 3. External Integration Data Flows

### 3.1 Prometheus Integration

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    Prometheus Query Flow                                 │
└─────────────────────────────────────────────────────────────────────────┘

Request:
┌─────────────────────────────────────────────────────────────────────────┐
│ GET /api/v1/query                                                        │
│ ?query=avg_over_time(openebs_volume_iops{volume_id="pvc-abc123"}[3600s])│
│                                                                          │
│ Headers:                                                                │
│   Content-Type: application/x-www-form-urlencoded                       │
│   Accept: application/json                                              │
└─────────────────────────────────────────────────────────────────────────┘

Response (Success):
┌─────────────────────────────────────────────────────────────────────────┐
│ HTTP/1.1 200 OK                                                          │
│ Content-Type: application/json                                          │
│                                                                          │
│ {                                                                        │
│   "status": "success",                                                  │
│   "data": {                                                             │
│     "resultType": "vector",                                             │
│     "result": [                                                         │
│       {                                                                 │
│         "metric": {                                                     │
│           "__name__": "openebs_volume_iops",                            │
│           "volume_id": "pvc-abc123",                                    │
│           "pool": "pool-sata-1"                                         │
│         },                                                              │
│         "value": [1706867400.123, "5234.567"]                           │
│       }                                                                 │
│     ]                                                                   │
│   }                                                                     │
│ }                                                                       │
└─────────────────────────────────────────────────────────────────────────┘

Response (Empty - No Data):
┌─────────────────────────────────────────────────────────────────────────┐
│ HTTP/1.1 200 OK                                                          │
│ {                                                                        │
│   "status": "success",                                                  │
│   "data": { "resultType": "vector", "result": [] }                      │
│ }                                                                       │
│                                                                          │
│ → Operator returns HeatScore::zero() (volume treated as cold)           │
└─────────────────────────────────────────────────────────────────────────┘
```

### 3.2 Kubernetes API Integration

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    Kubernetes API Calls                                  │
└─────────────────────────────────────────────────────────────────────────┘

Watch StoragePolicy:
┌─────────────────────────────────────────────────────────────────────────┐
│ GET /apis/storage.billyronks.io/v1/storagepolicies?watch=true           │
│                                                                          │
│ Event Stream:                                                           │
│ {"type":"ADDED","object":{...}}                                         │
│ {"type":"MODIFIED","object":{...}}                                      │
│ {"type":"DELETED","object":{...}}                                       │
└─────────────────────────────────────────────────────────────────────────┘

List PersistentVolumes:
┌─────────────────────────────────────────────────────────────────────────┐
│ GET /api/v1/persistentvolumes                                           │
│                                                                          │
│ Response: {"items": [...], "metadata": {"resourceVersion": "12345"}}    │
└─────────────────────────────────────────────────────────────────────────┘

Get MayastorVolume:
┌─────────────────────────────────────────────────────────────────────────┐
│ GET /apis/openebs.io/v1/namespaces/mayastor/mayastorvolumes/pvc-abc123  │
└─────────────────────────────────────────────────────────────────────────┘

Patch MayastorVolume:
┌─────────────────────────────────────────────────────────────────────────┐
│ PATCH /apis/openebs.io/v1/namespaces/mayastor/mayastorvolumes/pvc-abc123│
│ Content-Type: application/merge-patch+json                              │
│                                                                          │
│ {"spec": {"numReplicas": 2}}                                            │
└─────────────────────────────────────────────────────────────────────────┘

Patch StoragePolicy Status:
┌─────────────────────────────────────────────────────────────────────────┐
│ PATCH /apis/storage.billyronks.io/v1/storagepolicies/my-policy/status   │
│ Content-Type: application/merge-patch+json                              │
│                                                                          │
│ {"status": {"phase": "Active", "watchedVolumes": 50, ...}}              │
└─────────────────────────────────────────────────────────────────────────┘
```

## 4. Metrics Exposition Flow

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    Metrics Scrape Flow                                   │
└─────────────────────────────────────────────────────────────────────────┘

Request (from Prometheus):
┌─────────────────────────────────────────────────────────────────────────┐
│ GET /metrics HTTP/1.1                                                    │
│ Host: couchestor:8080                                       │
└─────────────────────────────────────────────────────────────────────────┘

Response:
┌─────────────────────────────────────────────────────────────────────────┐
│ HTTP/1.1 200 OK                                                          │
│ Content-Type: text/plain; version=0.0.4; charset=utf-8                  │
│                                                                          │
│ # HELP storage_operator_reconcile_total Total number of reconciliations │
│ # TYPE storage_operator_reconcile_total counter                         │
│ storage_operator_reconcile_total 1523                                   │
│                                                                          │
│ # HELP storage_operator_migrations_total Total number of migrations     │
│ # TYPE storage_operator_migrations_total counter                        │
│ storage_operator_migrations_total{status="success"} 145                 │
│ storage_operator_migrations_total{status="failed"} 3                    │
│ storage_operator_migrations_total{status="aborted"} 2                   │
│                                                                          │
│ # HELP storage_operator_active_migrations Current active migrations     │
│ # TYPE storage_operator_active_migrations gauge                         │
│ storage_operator_active_migrations 1                                    │
└─────────────────────────────────────────────────────────────────────────┘
```

## 5. Data Retention

| Data Type | Retention | Storage |
|-----------|-----------|---------|
| Heat Score Cache | 30 seconds (TTL) | In-memory (DashMap) |
| Active Migrations | Until completion | In-memory (DashMap) |
| Migration History | Last 50 entries | StoragePolicy Status |
| Metrics | Prometheus scrape interval | Prometheus |
| Logs | Cluster logging policy | Stdout/JSON |

## 6. Error Data Flows

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    Error Propagation                                     │
└─────────────────────────────────────────────────────────────────────────┘

Prometheus Error:
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  Prometheus  │ ──▶ │   Metrics    │ ──▶ │  Controller  │
│  Timeout     │     │   Watcher    │     │              │
└──────────────┘     └──────────────┘     └──────────────┘
                           │                     │
                           ▼                     ▼
                     Return Zero Score     Log Warning,
                     Mark Unhealthy        Continue with
                                           Zero Score

Migration Error:
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  K8s API     │ ──▶ │   Migrator   │ ──▶ │  Controller  │
│  Error       │     │              │     │              │
└──────────────┘     └──────────────┘     └──────────────┘
                           │                     │
                           ▼                     ▼
                     Abort Migration,      Log Error,
                     Preserve Data,        Update Status,
                     Return Error          Requeue (60s)
```
