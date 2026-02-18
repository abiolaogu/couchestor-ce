# Use Cases Document — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Use Case Actors

| Actor | Description |
|-------|-------------|
| Platform Admin | Manages Kubernetes cluster and storage infrastructure |
| Storage Operator | Configures storage policies and monitors tiering |
| Application Developer | Deploys workloads that consume Mayastor volumes |
| SRE/DevOps | Monitors system health and responds to incidents |
| CoucheStor Operator (System) | Automated K8s controller performing tiering decisions |

## 2. Core Use Cases

### UC-001: Configure Automated Storage Tiering
**Actor**: Storage Operator
**Precondition**: CoucheStor operator deployed, Prometheus running, Mayastor pools labeled
**Trigger**: Storage Operator wants to automate data placement

**Main Flow**:
1. Storage Operator creates StoragePolicy CRD defining IOPS thresholds
2. CoucheStor validates the policy (watermarks, selectors, durations)
3. CoucheStor begins watching volumes matching the policy's StorageClass
4. CoucheStor queries Prometheus for volume IOPS metrics
5. CoucheStor classifies volumes into Hot/Warm/Cold tiers
6. CoucheStor migrates misplaced volumes to their optimal tier
7. StoragePolicy status updated with migration counts and history

**Postcondition**: Volumes are automatically tiered based on IOPS patterns

**Alternate Flows**:
- 2a. Invalid policy: CoucheStor sets phase=Error with validation message
- 4a. Prometheus unavailable: CoucheStor logs warning, no migrations triggered
- 6a. Migration fails: Old replica preserved, error logged, retry on next reconciliation

### UC-002: Migrate Volume from Hot to Cold Tier
**Actor**: CoucheStor Operator (System)
**Precondition**: Volume IOPS below lowWatermark for samplingWindow, cooldown expired
**Trigger**: Controller reconciliation detects cold volume on hot tier

**Main Flow**:
1. System verifies volume is Online via Mayastor API
2. System selects target cold pool matching coldPoolSelector labels
3. System adds replica on cold pool (SCALE UP)
4. System polls replica status until Online+Synced (WAIT SYNC)
5. System removes hot tier replica (SCALE DOWN)
6. System updates StoragePolicy migration history
7. System emits MigrationCompleted domain event

**Postcondition**: Volume now served from cold tier storage

**Alternate Flows**:
- 2a. No cold pool available: Error::NoSuitablePool, abort
- 4a. Sync timeout: Migration aborted, both replicas preserved
- 5a. Preservation mode: Old replica kept, volume has extra replica

### UC-003: Enable Erasure Coding for Cold Storage
**Actor**: Storage Operator
**Precondition**: StoragePolicy exists, volumes on cold tier

**Main Flow**:
1. Storage Operator creates ErasureCodingPolicy with 4+2 config
2. CoucheStor validates EC policy (shards, stripe size, algorithm)
3. Storage Operator updates StoragePolicy with ecPolicyRef
4. When volumes migrate to cold tier, StripeManager activates
5. StripeManager reads volume data in 1MB stripe chunks
6. EcEncoder splits into 4 data + 2 parity shards
7. Shards distributed across different pools/nodes
8. ECStripe CRDs created to track stripe metadata

**Postcondition**: Cold tier volumes use 50% storage overhead instead of 200%

### UC-004: Handle Shard Failure and Reconstruction
**Actor**: CoucheStor Operator (System)
**Precondition**: EC-encoded volume with healthy stripes
**Trigger**: Storage node or disk failure causing shard loss

**Main Flow**:
1. System detects missing shard (periodic health check or read failure)
2. System identifies affected ECStripe CRDs
3. System marks affected stripes as Degraded
4. ReconstructionEngine reads surviving shards
5. Reed-Solomon reconstruction fills in missing data
6. Reconstructed shards written to new healthy pools
7. ECStripe CRDs updated with new shard locations
8. Stripe status set back to Healthy

**Postcondition**: All stripes restored to full health

**Alternate Flows**:
- 4a. More than m shards lost: Stripe marked Failed, alert emitted
- 6a. No healthy pool available: Reconstruction delayed until pool available

### UC-005: Perform Degraded Read
**Actor**: Application Developer (indirect, via I/O path)
**Precondition**: EC-encoded volume with degraded stripe
**Trigger**: Read request for LBA in degraded stripe

**Main Flow**:
1. Application reads from volume
2. CoucheStor identifies stripe for requested LBA range
3. System attempts to read all k data shards
4. Some shards are unavailable (< m missing)
5. System reads available data + parity shards
6. Reed-Solomon decodes reconstructs missing data
7. Reconstructed data returned to application transparently
8. DegradedRead domain event emitted

**Postcondition**: Application receives data without errors despite shard failure

### UC-006: Test Policy with Dry-Run Mode
**Actor**: Storage Operator
**Precondition**: CoucheStor operator deployed

**Main Flow**:
1. Storage Operator creates StoragePolicy with dryRun=true
2. CoucheStor watches volumes and queries metrics normally
3. CoucheStor logs migration decisions to stdout/stderr
4. No actual migrations are executed
5. Storage Operator reviews logs to validate policy thresholds
6. Storage Operator sets dryRun=false to activate migrations

**Postcondition**: Policy validated without risk to production data

### UC-007: Monitor Operator Health
**Actor**: SRE/DevOps
**Precondition**: CoucheStor deployed with health and metrics services

**Main Flow**:
1. Kubernetes kubelet queries /healthz for liveness probe
2. Kubernetes kubelet queries /readyz for readiness probe
3. Prometheus scrapes :8080/metrics for operational metrics
4. SRE reviews couchestor_migrations_total for migration rates
5. SRE reviews couchestor_active_migrations for in-flight count
6. SRE sets up alerting rules for migration failures

**Postcondition**: Operator health and performance continuously monitored

### UC-008: Discover Hardware for Tier Assignment
**Actor**: Platform Admin
**Precondition**: Storage nodes with NVMe, SAS, SATA devices
**Trigger**: Initial cluster setup or node addition

**Main Flow**:
1. Platform Admin triggers hardware scan (via CoucheStor CLI or API)
2. HardwareScanner enumerates block devices from sysfs
3. NVMe devices identified with controller and namespace info
4. SAS/SATA devices identified with model and capacity
5. SMART data retrieved for health assessment
6. NodeHardwareInfo compiled with drive inventory
7. Admin uses info to label DiskPools with tier labels

**Postcondition**: Storage infrastructure fully inventoried for tier configuration

### UC-009: Configure Three-Tier Caching
**Actor**: Application Developer
**Precondition**: CoucheStor library integrated (src/rustfs/cache/)

**Main Flow**:
1. Developer creates CacheManager with configuration
2. CacheManager initializes L1 (RAM, 50GB), L2 (NVMe, 500GB), L3 (Cold, 10TB+)
3. Application stores objects via CacheManager.put(key, entry)
4. L1 stores hot data with sub-microsecond access
5. L2 stores warm data with sub-100us access via mmap
6. L3 stores cold data with sub-10ms access
7. CacheManager automatically promotes/demotes between tiers
8. CacheMetrics tracks hit ratios per tier

**Postcondition**: Application benefits from tiered caching with optimal latency

### UC-010: Emergency Stop All Migrations
**Actor**: SRE/DevOps
**Precondition**: Active migrations causing issues
**Trigger**: Incident detection

**Main Flow**:
1. SRE patches StoragePolicy: `spec.enabled: false`
2. CoucheStor sets policy phase to Disabled
3. No new migrations are initiated
4. In-flight migrations complete (data safety preserved)
5. SRE investigates root cause
6. SRE re-enables policy after resolution

**Postcondition**: All automated migrations stopped, data safe

## 3. Use Case Diagram

```
              ┌────────────────────┐
              │  Storage Operator  │
              └─────┬───┬───┬─────┘
                    │   │   │
        ┌───────────┘   │   └───────────┐
        ▼               ▼               ▼
  ┌──────────┐   ┌──────────┐   ┌──────────┐
  │UC-001    │   │UC-003    │   │UC-006    │
  │Configure │   │Enable EC │   │Dry-Run   │
  │Tiering   │   │for Cold  │   │Test      │
  └──────────┘   └──────────┘   └──────────┘

              ┌────────────────────┐
              │ CoucheStor System  │
              └─────┬───┬───┬─────┘
                    │   │   │
        ┌───────────┘   │   └───────────┐
        ▼               ▼               ▼
  ┌──────────┐   ┌──────────┐   ┌──────────┐
  │UC-002    │   │UC-004    │   │UC-005    │
  │Migrate   │   │Shard     │   │Degraded  │
  │Volume    │   │Reconstruct│  │Read      │
  └──────────┘   └──────────┘   └──────────┘

              ┌────────────────────┐
              │    SRE/DevOps     │
              └─────┬───┬─────────┘
                    │   │
        ┌───────────┘   └───────────┐
        ▼                           ▼
  ┌──────────┐               ┌──────────┐
  │UC-007    │               │UC-010    │
  │Monitor   │               │Emergency │
  │Health    │               │Stop      │
  └──────────┘               └──────────┘
```

## 4. Use Case Priority Matrix

| Use Case | Priority | Complexity | Implemented |
|----------|----------|------------|-------------|
| UC-001: Configure Tiering | P0 | Medium | Yes |
| UC-002: Migrate Volume | P0 | High | Yes |
| UC-003: Enable EC | P0 | High | Yes |
| UC-004: Reconstruct Shards | P1 | High | Yes |
| UC-005: Degraded Read | P1 | Medium | Yes |
| UC-006: Dry-Run Test | P1 | Low | Yes |
| UC-007: Monitor Health | P0 | Low | Yes |
| UC-008: Hardware Discovery | P2 | Medium | Yes |
| UC-009: Three-Tier Cache | P1 | High | Yes |
| UC-010: Emergency Stop | P0 | Low | Yes |
