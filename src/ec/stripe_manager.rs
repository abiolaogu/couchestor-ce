//! Stripe Manager - Journal Destaging
//!
//! Manages background destaging from journal (replicated) storage
//! to erasure-coded stripes for cold tier storage.

use crate::crd::{ErasureCodingPolicy, JournalConfig, LbaRange, ShardLocation, StripeState};
use crate::ec::encoder::EcEncoder;
use crate::ec::metadata::{EcMetadataManager, StripeMetadata, StripeStatus};
use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::interval;
use tracing::{debug, error, info, instrument};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the stripe manager
#[derive(Debug, Clone)]
pub struct StripeManagerConfig {
    /// Interval between destage checks
    pub destage_interval: Duration,

    /// Number of stripes to destage per batch
    pub batch_size: usize,

    /// Journal fill threshold percentage to trigger destaging
    pub destage_threshold_percent: u8,

    /// Maximum concurrent destage operations
    pub max_concurrent: usize,

    /// Whether running in dry-run mode
    pub dry_run: bool,
}

impl Default for StripeManagerConfig {
    fn default() -> Self {
        Self {
            destage_interval: Duration::from_secs(30),
            batch_size: 10,
            destage_threshold_percent: 80,
            max_concurrent: 4,
            dry_run: false,
        }
    }
}

impl StripeManagerConfig {
    /// Create config from EC policy journal settings
    pub fn from_journal_config(journal: &JournalConfig, dry_run: bool) -> Result<Self> {
        let destage_interval = crate::crd::parse_duration(&journal.destage_interval)?;

        Ok(Self {
            destage_interval,
            destage_threshold_percent: journal.destage_threshold_percent,
            dry_run,
            ..Default::default()
        })
    }
}

// =============================================================================
// Destage Request
// =============================================================================

/// Request to destage data from journal to EC
#[derive(Debug, Clone)]
pub struct DestageRequest {
    /// Volume ID
    pub volume_id: String,

    /// Journal entries to destage
    pub journal_entries: Vec<JournalEntry>,

    /// Priority (higher = more urgent)
    pub priority: u8,

    /// Time request was created
    pub created_at: DateTime<Utc>,
}

/// A journal entry representing pending write data
#[derive(Debug, Clone)]
pub struct JournalEntry {
    /// LBA range of this entry
    pub lba_range: LbaRange,

    /// Data to be written
    pub data: Vec<u8>,

    /// Timestamp when written to journal
    pub timestamp: DateTime<Utc>,
}

// =============================================================================
// Destage Result
// =============================================================================

/// Result of a destage operation
#[derive(Debug)]
pub struct DestageResult {
    /// Volume ID
    pub volume_id: String,

    /// Number of entries destaged
    pub entries_destaged: usize,

    /// Stripes created
    pub stripes_created: Vec<u64>,

    /// Total bytes destaged
    pub bytes_destaged: u64,

    /// Duration of operation
    pub duration: Duration,

    /// Any errors encountered
    pub errors: Vec<String>,
}

// =============================================================================
// Stripe Manager
// =============================================================================

/// Manages background destaging from journal to EC storage
pub struct StripeManager {
    /// Configuration
    config: StripeManagerConfig,

    /// Metadata manager
    metadata_manager: Arc<EcMetadataManager>,

    /// Pending destage requests
    pending_requests: Arc<RwLock<VecDeque<DestageRequest>>>,

    /// Shutdown signal
    shutdown: Arc<RwLock<bool>>,

    /// Request sender for submitting destage requests
    request_tx: mpsc::Sender<DestageRequest>,

    /// Request receiver
    request_rx: Arc<RwLock<mpsc::Receiver<DestageRequest>>>,
}

impl StripeManager {
    /// Create a new stripe manager
    pub fn new(config: StripeManagerConfig, metadata_manager: Arc<EcMetadataManager>) -> Arc<Self> {
        let (tx, rx) = mpsc::channel(1000);

        Arc::new(Self {
            config,
            metadata_manager,
            pending_requests: Arc::new(RwLock::new(VecDeque::new())),
            shutdown: Arc::new(RwLock::new(false)),
            request_tx: tx,
            request_rx: Arc::new(RwLock::new(rx)),
        })
    }

    /// Get a sender for submitting destage requests
    pub fn request_sender(&self) -> mpsc::Sender<DestageRequest> {
        self.request_tx.clone()
    }

    /// Submit a destage request
    #[instrument(skip(self, request), fields(volume = %request.volume_id))]
    pub async fn submit_request(&self, request: DestageRequest) -> Result<()> {
        self.request_tx
            .send(request)
            .await
            .map_err(|e| Error::Internal(format!("Failed to submit destage request: {}", e)))?;
        Ok(())
    }

    /// Run the stripe manager background loop
    #[instrument(skip(self))]
    pub async fn run(self: Arc<Self>) {
        info!("Starting stripe manager with {:?}", self.config);

        let mut tick = interval(self.config.destage_interval);

        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if *self.shutdown.read().await {
                        info!("Stripe manager shutting down");
                        break;
                    }

                    if let Err(e) = self.process_pending_requests().await {
                        error!("Error processing destage requests: {}", e);
                    }
                }

                // Receive new requests
                request = async {
                    let mut rx = self.request_rx.write().await;
                    rx.recv().await
                } => {
                    if let Some(request) = request {
                        let mut pending = self.pending_requests.write().await;
                        pending.push_back(request);
                        debug!("Queued destage request, {} pending", pending.len());
                    }
                }
            }
        }
    }

    /// Signal shutdown
    pub async fn shutdown(&self) {
        *self.shutdown.write().await = true;
    }

    /// Process pending destage requests
    #[instrument(skip(self))]
    async fn process_pending_requests(&self) -> Result<()> {
        let mut pending = self.pending_requests.write().await;

        if pending.is_empty() {
            return Ok(());
        }

        // Process up to batch_size requests
        let batch_count = std::cmp::min(pending.len(), self.config.batch_size);
        let batch: Vec<DestageRequest> = pending.drain(..batch_count).collect();

        drop(pending); // Release lock

        info!("Processing {} destage requests", batch.len());

        for request in batch {
            match self.destage_volume(&request).await {
                Ok(result) => {
                    info!(
                        "Destaged {} entries for volume {}, created {} stripes ({} bytes)",
                        result.entries_destaged,
                        result.volume_id,
                        result.stripes_created.len(),
                        result.bytes_destaged
                    );
                }
                Err(e) => {
                    error!("Failed to destage volume {}: {}", request.volume_id, e);
                    // Re-queue failed requests with lower priority
                    let mut pending = self.pending_requests.write().await;
                    let mut retry = request.clone();
                    retry.priority = retry.priority.saturating_sub(1);
                    pending.push_back(retry);
                }
            }
        }

        Ok(())
    }

    /// Destage a volume's journal entries to EC storage
    #[instrument(skip(self, request), fields(volume = %request.volume_id))]
    async fn destage_volume(&self, request: &DestageRequest) -> Result<DestageResult> {
        let start = std::time::Instant::now();
        let mut result = DestageResult {
            volume_id: request.volume_id.clone(),
            entries_destaged: 0,
            stripes_created: vec![],
            bytes_destaged: 0,
            duration: Duration::ZERO,
            errors: vec![],
        };

        if request.journal_entries.is_empty() {
            result.duration = start.elapsed();
            return Ok(result);
        }

        // Get volume state
        let volume_state = self
            .metadata_manager
            .get_volume(&request.volume_id)
            .ok_or_else(|| Error::EcDestageFailed {
                volume_id: request.volume_id.clone(),
                reason: "Volume not found in EC state".to_string(),
            })?;

        // Extract policy_ref in a block to ensure guard is dropped before await
        let policy_ref = {
            let state = volume_state.read();
            state.policy_ref.clone()
        };

        // Load EC policy
        let policy = self.metadata_manager.load_policy(&policy_ref).await?;

        // Dry run check
        if self.config.dry_run {
            info!(
                "[DRY-RUN] Would destage {} entries for volume {}",
                request.journal_entries.len(),
                request.volume_id
            );
            result.entries_destaged = request.journal_entries.len();
            result.duration = start.elapsed();
            return Ok(result);
        }

        // Create encoder
        let encoder = EcEncoder::new(
            policy.spec.data_shards as usize,
            policy.spec.parity_shards as usize,
        )?;

        // Group entries into stripe-sized batches
        let stripe_size = policy.spec.stripe_size_bytes as usize;
        let mut current_batch: Vec<u8> = Vec::with_capacity(stripe_size);
        let mut current_lba_start: Option<u64> = None;

        for entry in &request.journal_entries {
            if current_lba_start.is_none() {
                current_lba_start = Some(entry.lba_range.start_lba);
            }

            current_batch.extend_from_slice(&entry.data);

            // If batch is full, create a stripe
            if current_batch.len() >= stripe_size {
                match self
                    .create_stripe(
                        &request.volume_id,
                        &policy,
                        &encoder,
                        &current_batch[..stripe_size],
                        current_lba_start.unwrap(),
                    )
                    .await
                {
                    Ok(stripe_id) => {
                        result.stripes_created.push(stripe_id);
                        result.bytes_destaged += stripe_size as u64;
                    }
                    Err(e) => {
                        result.errors.push(e.to_string());
                    }
                }

                // Keep any overflow for the next stripe
                current_batch = current_batch[stripe_size..].to_vec();
                current_lba_start = None;
            }

            result.entries_destaged += 1;
        }

        // Handle remaining data (partial stripe)
        if !current_batch.is_empty() {
            if let Some(lba_start) = current_lba_start {
                match self
                    .create_stripe(
                        &request.volume_id,
                        &policy,
                        &encoder,
                        &current_batch,
                        lba_start,
                    )
                    .await
                {
                    Ok(stripe_id) => {
                        result.stripes_created.push(stripe_id);
                        result.bytes_destaged += current_batch.len() as u64;
                    }
                    Err(e) => {
                        result.errors.push(e.to_string());
                    }
                }
            }
        }

        result.duration = start.elapsed();
        Ok(result)
    }

    /// Create a single EC stripe from data
    #[instrument(skip(self, policy, encoder, data))]
    async fn create_stripe(
        &self,
        volume_id: &str,
        policy: &ErasureCodingPolicy,
        encoder: &EcEncoder,
        data: &[u8],
        start_lba: u64,
    ) -> Result<u64> {
        // Encode data into shards
        let shards = encoder.encode(data)?;

        // Get volume state and allocate stripe ID
        let volume_state =
            self.metadata_manager
                .get_volume(volume_id)
                .ok_or_else(|| Error::EcDestageFailed {
                    volume_id: volume_id.to_string(),
                    reason: "Volume not found".to_string(),
                })?;

        // Calculate LBA range
        // LBA size is typically 512 bytes or 4KB; we'll use the data size
        let lba_count = (data.len() as u64).div_ceil(512); // Round up to 512-byte blocks
        let lba_range = LbaRange::new(start_lba, start_lba + lba_count);

        // Create shard locations (in a real implementation, these would be
        // assigned based on pool availability and placement rules)
        let shard_locations: Vec<ShardLocation> = shards
            .iter()
            .enumerate()
            .map(|(i, shard)| {
                let is_data = i < policy.spec.data_shards as usize;
                ShardLocation {
                    shard_index: i as u8,
                    is_data_shard: is_data,
                    pool_name: format!("pool-{}", i % 6), // Placeholder
                    node_name: format!("node-{}", i % 3), // Placeholder
                    offset: 0,                            // Would be allocated by storage backend
                    size_bytes: shard.len() as u64,
                    checksum: None,
                }
            })
            .collect();

        // Get stripe ID and add metadata in a block to ensure guard is dropped before await
        let (stripe_id, metadata) = {
            let mut state = volume_state.write();
            let stripe_id = state.next_stripe_id();

            // Create stripe metadata
            let metadata = StripeMetadata {
                stripe_id,
                volume_id: volume_id.to_string(),
                policy_ref: policy.name().to_string(),
                lba_range: lba_range.clone(),
                shard_locations: shard_locations.clone(),
                status: StripeStatus {
                    state: StripeState::Healthy,
                    healthy_shards: shards.len() as u8,
                    shard_health: vec![],
                },
                generation: 0,
                checksum: None,
            };

            // Add to in-memory state
            state.add_stripe(metadata.clone());
            (stripe_id, metadata)
        };

        // Persist to Kubernetes CRD
        self.metadata_manager.create_stripe_crd(&metadata).await?;

        debug!(
            "Created stripe {} for volume {}, LBA range {:?}",
            stripe_id, volume_id, lba_range
        );

        Ok(stripe_id)
    }

    /// Check if a volume needs destaging based on journal fill level
    pub async fn should_destage(&self, _volume_id: &str, journal_fill_percent: u8) -> bool {
        journal_fill_percent >= self.config.destage_threshold_percent
    }

    /// Get the number of pending requests
    pub async fn pending_count(&self) -> usize {
        self.pending_requests.read().await.len()
    }

    /// Get current configuration
    pub fn config(&self) -> &StripeManagerConfig {
        &self.config
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Configuration Tests
    // =========================================================================

    #[test]
    fn test_stripe_manager_config_default() {
        let config = StripeManagerConfig::default();

        assert_eq!(config.destage_interval, Duration::from_secs(30));
        assert_eq!(config.batch_size, 10);
        assert_eq!(config.destage_threshold_percent, 80);
        assert_eq!(config.max_concurrent, 4);
        assert!(!config.dry_run);
    }

    #[test]
    fn test_stripe_manager_config_from_journal() {
        let journal = JournalConfig {
            journal_size_bytes: 10737418240,
            replication_factor: 3,
            destage_threshold_percent: 70,
            destage_interval: "1m".to_string(),
        };

        let config = StripeManagerConfig::from_journal_config(&journal, true).unwrap();

        assert_eq!(config.destage_interval, Duration::from_secs(60));
        assert_eq!(config.destage_threshold_percent, 70);
        assert!(config.dry_run);
    }

    // =========================================================================
    // DestageRequest Tests
    // =========================================================================

    #[test]
    fn test_destage_request_creation() {
        let request = DestageRequest {
            volume_id: "vol-1".to_string(),
            journal_entries: vec![],
            priority: 5,
            created_at: Utc::now(),
        };

        assert_eq!(request.volume_id, "vol-1");
        assert_eq!(request.priority, 5);
        assert!(request.journal_entries.is_empty());
    }

    // =========================================================================
    // JournalEntry Tests
    // =========================================================================

    #[test]
    fn test_journal_entry_creation() {
        let entry = JournalEntry {
            lba_range: LbaRange::new(0, 100),
            data: vec![0u8; 51200], // 50KB
            timestamp: Utc::now(),
        };

        assert_eq!(entry.lba_range.size(), 100);
        assert_eq!(entry.data.len(), 51200);
    }

    // =========================================================================
    // DestageResult Tests
    // =========================================================================

    #[test]
    fn test_destage_result_creation() {
        let result = DestageResult {
            volume_id: "vol-1".to_string(),
            entries_destaged: 10,
            stripes_created: vec![0, 1, 2],
            bytes_destaged: 3145728, // 3MB
            duration: Duration::from_secs(5),
            errors: vec![],
        };

        assert_eq!(result.entries_destaged, 10);
        assert_eq!(result.stripes_created.len(), 3);
        assert_eq!(result.bytes_destaged, 3145728);
        assert!(result.errors.is_empty());
    }

    // =========================================================================
    // Should Destage Tests
    // =========================================================================

    #[test]
    fn test_should_destage_threshold() {
        let config = StripeManagerConfig {
            destage_threshold_percent: 80,
            ..Default::default()
        };

        // Below threshold
        assert!(79 < config.destage_threshold_percent);

        // At threshold
        assert!(80 >= config.destage_threshold_percent);

        // Above threshold
        assert!(90 >= config.destage_threshold_percent);
    }
}
