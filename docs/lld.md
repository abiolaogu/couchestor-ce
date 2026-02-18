# Low-Level Design — CoucheStor Community Edition
> Version: 1.0 | Last Updated: 2026-02-17 | Status: Draft
> Classification: Internal | Author: AIDD System

## 1. Module-Level Design

### 1.1 Controller Module (`src/controller/`)

#### 1.1.1 storage_policy.rs — StoragePolicy Reconciler

**Entry Point**: `pub async fn run(ctx: ControllerContext) -> Result<()>`

**Algorithm**:
```
1. Create kube::Api<StoragePolicy> (cluster-scoped)
2. Start watcher with kube::runtime::Controller
3. For each reconcile event:
   a. Fetch StoragePolicy by name
   b. If not found → return (deleted)
   c. If !enabled → set status.phase = Disabled, return
   d. List PVs matching storageClassName
   e. Filter by volumeSelector labels
   f. For each PV:
      i.   Query MetricsWatcher.get_heat_score(volume_id)
      ii.  Classify: HeatScore.classify(highWatermark, lowWatermark)
      iii. Get current tier from DiskPool labels
      iv.  If tier change needed:
           - Check cooldown: last_migration_time + cooldownPeriod > now?
           - Check semaphore: active_migrations < maxConcurrent?
           - If dry_run: log and skip
           - Select target pool using tier selector (hot/warm/cold)
           - Spawn migration task via Migrator
   g. Update status: watchedVolumes, hotVolumes, warmVolumes, coldVolumes
   h. Return ReconcilerAction::requeue(Duration::from_secs(60))
```

#### 1.1.2 ec_policy.rs — ErasureCodingPolicy Reconciler

**Entry Point**: `pub async fn run_ec_policy(ctx: EcPolicyContext) -> Result<()>`

**Algorithm**:
```
1. Create kube::Api<ErasureCodingPolicy> (cluster-scoped)
2. Start watcher
3. For each reconcile event:
   a. Fetch ErasureCodingPolicy by name
   b. Validate configuration:
      - dataShards > 0
      - parityShards > 0
      - stripeSizeBytes in [64KB, 64MB]
   c. If invalid → set status.phase = Invalid, set message
   d. If valid → set status.phase = Ready
   e. Compute storageEfficiency = k / (k+m)
   f. Update status
```

### 1.2 Metrics Module (`src/metrics/watcher.rs`)

**Struct**: `MetricsWatcher`
```rust
pub struct MetricsWatcher {
    client: reqwest::Client,
    config: MetricsConfig,
    cache: DashMap<String, CachedMetric>,
}
```

**Key Methods**:
- `new(config) -> Result<Self>`: Create HTTP client with timeout
- `health_check() -> Result<()>`: GET `{prometheus_url}/-/healthy`
- `query_volume_iops(volume_id) -> Result<f64>`:
  ```
  1. Check cache (DashMap lookup)
  2. If cache hit and TTL valid → return cached value
  3. Build PromQL: rate({metric_name}{volume="{volume_id}"}[{window}])
  4. POST to {prometheus_url}/api/v1/query
  5. Parse response JSON: data.result[0].value[1]
  6. Cache result with TTL
  7. If primary metric fails, try fallback_metrics in order
  ```

### 1.3 Migrator Module (`src/migrator/engine.rs`)

**Struct**: `Migrator`
```rust
pub struct Migrator {
    config: MigratorConfig,
    client: Client,
    active: DashMap<String, MigrationState>,
    semaphore: Arc<tokio::sync::Semaphore>,
}
```

**Migration Implementation (4-Step)**:
```rust
async fn migrate(&self, volume_id: &str, from_pool: &str, to_pool: &str) -> Result<()> {
    // Step 1: ANALYZE
    let volume = self.get_mayastor_volume(volume_id).await?;
    ensure!(volume.status.state == VolumeState::Online);
    let target_pool = self.get_disk_pool(to_pool).await?;
    ensure!(target_pool.is_online());
    ensure!(target_pool.status.available >= volume.spec.size);

    // Step 2: SCALE UP
    let new_replica = self.add_replica(volume_id, to_pool).await?;

    // Step 3: WAIT SYNC
    let synced = self.wait_for_sync(
        volume_id,
        &new_replica.uuid,
        self.config.sync_timeout,
        self.config.sync_poll_interval,
    ).await?;
    if !synced {
        return Err(Error::ReplicaSyncFailed("timeout".into()));
    }

    // Step 4: SCALE DOWN
    if !self.config.preservation_mode {
        let old_replica = volume.replicas().iter()
            .find(|r| r.pool == from_pool)
            .ok_or(Error::Internal("old replica not found".into()))?;
        self.remove_replica(volume_id, &old_replica.uuid).await?;
    }

    Ok(())
}
```

### 1.4 EC Encoder Module (`src/ec/encoder.rs`)

**Struct**: `EcEncoder`
```rust
pub struct EcEncoder {
    data_shards: usize,
    parity_shards: usize,
    encoder: ReedSolomon<galois_8::Field>,
}
```

**Encoding Algorithm**:
```
1. Input: raw data bytes
2. Calculate shard_size = ceil(data_len / k)
3. Pad data to k * shard_size
4. Split into k data shards of shard_size each
5. Allocate m parity shards (zeroed, same size)
6. Call encoder.encode(&mut shards) — Reed-Solomon computation
7. Return EncodedData { data_shards, parity_shards, original_len }
```

**Decoding Algorithm**:
```
1. Input: shards[Option<Vec<u8>>], original_len
2. Count missing shards
3. If missing > m → Error::InsufficientShards
4. Call encoder.reconstruct(&mut shards) — fills in missing
5. Concatenate first k shards (data shards)
6. Truncate to original_len
7. Return recovered data
```

### 1.5 Cache Module (`src/rustfs/cache/`)

#### 1.5.1 Shard Implementation (`shard.rs`)
```rust
pub struct ShardedMap<K, V> {
    shards: Vec<parking_lot::RwLock<HashMap<K, V>>>,
    shard_count: usize,  // Always 1024
}

impl<K: Hash, V> ShardedMap<K, V> {
    fn shard_index(&self, key: &K) -> usize {
        // Fast modulo via bitwise AND (1024 is power of 2)
        let hash = self.hash(key);
        hash as usize & (self.shard_count - 1)
    }

    fn get(&self, key: &K) -> Option<&V> {
        let idx = self.shard_index(key);
        let guard = self.shards[idx].read();  // parking_lot RwLock
        guard.get(key)
    }

    fn insert(&self, key: K, value: V) {
        let idx = self.shard_index(&key);
        let mut guard = self.shards[idx].write();
        guard.insert(key, value);
    }
}
```

#### 1.5.2 L1 Cache (`l1.rs`)
```rust
pub struct L1Cache {
    data: ShardedMap<CacheKey, CacheEntry>,
    capacity_bytes: u64,       // Default: 50GB
    current_size: AtomicU64,
    metrics: CacheMetrics,
}
```

Operations: O(1) get/put via sharded hash map. Eviction: LRU per shard.

#### 1.5.3 L2 Cache (`l2.rs`)
```rust
pub struct L2Cache {
    base_path: PathBuf,            // NVMe mount point
    index: ShardedMap<CacheKey, L2Entry>,
    capacity_bytes: u64,           // Default: 500GB
    current_size: AtomicU64,
}

struct L2Entry {
    file_path: PathBuf,
    offset: u64,
    size: u64,
    checksum: u32,
}
```

Uses memory-mapped files for kernel page cache utilization. Minimum entry size: 4KB (L2_MIN_ENTRY_SIZE).

#### 1.5.4 Compression (`compression.rs`)
```rust
pub enum CompressionAlgorithm {
    None,
    Lz4,   // CE only
}

pub struct CompressionManager {
    algorithm: CompressionAlgorithm,
    min_size: usize,  // Don't compress below this size
}

impl CompressionManager {
    pub fn compress(&self, data: &[u8]) -> Result<Vec<u8>> {
        match self.algorithm {
            CompressionAlgorithm::None => Ok(data.to_vec()),
            CompressionAlgorithm::Lz4 => {
                lz4::block::compress(data, None, true)
                    .map_err(|e| Error::CompressionFailed {
                        algorithm: "LZ4".into(),
                        reason: e.to_string(),
                    })
            }
        }
    }
}
```

### 1.6 SPDK Module (`src/spdk/`)

#### 1.6.1 DMA Buffer (`dma_buf.rs`)
```rust
pub struct DmaBuf {
    ptr: *mut u8,
    len: usize,
    alignment: usize,  // SPDK_DMA_ALIGNMENT (typically 4096)
}

impl DmaBuf {
    pub fn allocate(size: usize) -> Result<Self> {
        // Calls spdk_dma_malloc via FFI
        let ptr = unsafe { ffi::spdk_dma_malloc(size, SPDK_DMA_ALIGNMENT, std::ptr::null_mut()) };
        if ptr.is_null() {
            return Err(Error::DmaAllocationFailed { size, reason: "spdk_dma_malloc returned null".into() });
        }
        Ok(Self { ptr: ptr as *mut u8, len: size, alignment: SPDK_DMA_ALIGNMENT })
    }
}

impl Drop for DmaBuf {
    fn drop(&mut self) {
        unsafe { ffi::spdk_dma_free(self.ptr as *mut libc::c_void) }
    }
}
```

#### 1.6.2 ISA-L Codec (`isal_codec.rs`)
```rust
pub struct IsalCodec {
    config: IsalCodecConfig,
    encode_matrix: Vec<u8>,      // Cauchy matrix
    encode_tables: Vec<u8>,      // Pre-computed GF tables
}

impl IsalCodec {
    pub fn encode(&self, data_bufs: &[DmaBuf], parity_bufs: &mut [DmaBuf]) -> Result<()> {
        // Uses ISA-L ec_encode_data for hardware-accelerated encoding
        // Detects AVX2/AVX-512 at runtime for optimal SIMD path
        unsafe {
            ffi::ec_encode_data(
                self.config.stripe_size as i32,
                self.config.data_shards as i32,
                self.config.parity_shards as i32,
                self.encode_tables.as_ptr(),
                data_ptrs.as_ptr(),
                parity_ptrs.as_mut_ptr(),
            );
        }
        Ok(())
    }
}
```

### 1.7 Hardware Discovery Module (`src/hardware/discovery/`)

#### 1.7.1 Scanner (`scanner.rs`)
```rust
pub struct HardwareScanner {
    config: ScannerConfig,
}

pub struct ScannerConfig {
    sysfs_block_path: PathBuf,    // Default: /sys/class/block
    sysfs_nvme_path: PathBuf,     // Default: /sys/class/nvme
    nvme_cli_path: PathBuf,       // Default: /usr/sbin/nvme
    smartctl_path: PathBuf,       // Default: /usr/sbin/smartctl
}
```

Discovery uses `tokio::process::Command` for external tools (nvme-cli, smartctl) and direct sysfs reads for device enumeration.

## 2. Error Handling Design

### 2.1 Error Type Hierarchy
```rust
pub enum Error {
    // Kubernetes (auto from kube::Error)
    Kube(kube::Error),
    Io(std::io::Error),

    // Prometheus (3 variants: connection, query, parse)
    PrometheusConnection(reqwest::Error),
    PrometheusQuery(String),
    PrometheusResponseParse(String),

    // Migration (4 variants: in_progress, failed, timeout, no_pool)
    MigrationInProgress { volume_name },
    MigrationFailed { volume_name, reason },
    MigrationTimeout { volume_name, duration },
    NoSuitablePool { tier },

    // EC (6 variants: encoding, reconstruction, shards, policy, stripe, config)
    EcEncodingFailed(String),
    EcReconstructionFailed { stripe_id, reason },
    InsufficientShards { available, required },

    // SPDK (5 variants: DMA, init, bdev, ISA-L matrix/encoding)
    DmaAllocationFailed { size, reason },

    // Hardware (3 variants: discovery, NVMe command, SMART)
    HardwareDiscovery(String),

    // Compression (2 variants: compress/decompress)
    CompressionFailed { algorithm, reason },
}
```

### 2.2 Error Propagation
- `Result<T>` = `std::result::Result<T, Error>` (module-level alias)
- `thiserror` for derive macros and automatic Display implementations
- `#[from]` attribute for automatic conversion from kube::Error, std::io::Error
- `#[source]` attribute for reqwest::Error chaining

## 3. Testing Design

### 3.1 Unit Test Structure
Each module contains `#[cfg(test)] mod tests` with tests co-located with implementation:
- `crd/storage_policy.rs`: 25+ tests for duration parsing, label selectors, conditions
- `crd/erasure_coding.rs`: 20+ tests for LBA ranges, stripe states, validation
- `crd/mayastor.rs`: 15+ tests for pool/volume/replica status
- `domain/ports.rs`: Tests for HeatScore classification, LBA range operations
- `domain/events.rs`: Tests for event serialization, builders
- `rustfs/cache/mod.rs`: Tests for shard count, alignment, capacities

### 3.2 Integration Tests
- `tests/ec_integration.rs`: End-to-end EC encode/decode/reconstruct
- `tests/integration_tests.rs`: Controller and migrator integration

### 3.3 Property-Based Tests
- `ec/proptest.rs`: Property-based testing for EC encoder using proptest crate
  - Arbitrary data → encode → lose up to m shards → reconstruct → verify
  - Ensures Reed-Solomon invariants hold for all inputs

## 4. Configuration Defaults

| Parameter | Default | Environment Variable |
|-----------|---------|---------------------|
| prometheus_url | http://prometheus.monitoring.svc.cluster.local:9090 | PROMETHEUS_URL |
| max_concurrent_migrations | 2 | MAX_CONCURRENT_MIGRATIONS |
| migration_timeout_minutes | 30 | MIGRATION_TIMEOUT_MINUTES |
| sync_poll_interval_seconds | 10 | SYNC_POLL_INTERVAL_SECONDS |
| dry_run | false | DRY_RUN |
| preservation_mode | false | PRESERVATION_MODE |
| mayastor_namespace | mayastor | MAYASTOR_NAMESPACE |
| metrics_addr | 0.0.0.0:8080 | METRICS_ADDR |
| health_addr | 0.0.0.0:8081 | HEALTH_ADDR |
| log_level | info | LOG_LEVEL |
| log_json | false | LOG_JSON |

## 5. Build Configuration

### 5.1 Release Profile
```toml
[profile.release]
lto = true            # Link-Time Optimization for small binary
codegen-units = 1     # Single codegen unit for max optimization
panic = "abort"       # No unwinding overhead
strip = true          # Strip debug symbols
```

### 5.2 Feature Flags
```toml
[features]
default = []          # No SPDK by default
spdk = ["libc"]       # Real SPDK/ISA-L
mock-spdk = []        # Mock for testing
ec-sidecar = ["spdk"] # Legacy alias
```
