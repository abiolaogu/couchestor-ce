//! Reconstruction Engine
//!
//! Handles degraded reads and background stripe rebuilds for
//! erasure-coded volumes.

use crate::crd::{LbaRange, ShardState, StripeState};
use crate::ec::encoder::EcDecoder;
use crate::ec::metadata::{EcMetadataManager, StripeMetadata};
use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};
use tokio::time::interval;
use tracing::{debug, error, info, instrument};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the reconstruction engine
#[derive(Debug, Clone)]
pub struct ReconstructionConfig {
    /// Maximum concurrent reconstructions
    pub max_concurrent: usize,

    /// Timeout for reading a single shard
    pub shard_read_timeout: Duration,

    /// Timeout for full stripe reconstruction
    pub stripe_timeout: Duration,

    /// Interval for background rebuild checks
    pub rebuild_check_interval: Duration,

    /// Whether to prioritize read requests over rebuilds
    pub prioritize_reads: bool,
}

impl Default for ReconstructionConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            shard_read_timeout: Duration::from_secs(30),
            stripe_timeout: Duration::from_secs(300), // 5 minutes
            rebuild_check_interval: Duration::from_secs(60),
            prioritize_reads: true,
        }
    }
}

// =============================================================================
// Reconstruction Task
// =============================================================================

/// A task representing an active reconstruction
#[derive(Debug, Clone)]
pub struct ReconstructionTask {
    /// Unique task ID
    pub task_id: u64,

    /// Volume ID
    pub volume_id: String,

    /// Stripe ID being reconstructed
    pub stripe_id: u64,

    /// Missing shard indices
    pub missing_shards: Vec<u8>,

    /// Task type
    pub task_type: ReconstructionType,

    /// Priority (higher = more urgent)
    pub priority: u8,

    /// When the task was created
    pub created_at: DateTime<Utc>,

    /// Current progress (0-100)
    pub progress: u8,

    /// Current status
    pub status: TaskStatus,
}

/// Type of reconstruction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconstructionType {
    /// Degraded read - reconstruct for immediate use
    DegradedRead,
    /// Background rebuild - restore redundancy
    BackgroundRebuild,
    /// Scrub verification - check data integrity
    ScrubVerification,
}

/// Status of a reconstruction task
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

// =============================================================================
// Read Request/Result
// =============================================================================

/// Request to read from an EC volume
#[derive(Debug, Clone)]
pub struct ReadRequest {
    /// Volume ID
    pub volume_id: String,

    /// LBA range to read
    pub lba_range: LbaRange,

    /// Whether to allow degraded reads
    pub allow_degraded: bool,
}

/// Result of a read operation
#[derive(Debug)]
pub struct ReadResult {
    /// The requested data
    pub data: Vec<u8>,

    /// Whether this was a degraded read
    pub degraded: bool,

    /// Stripes that were reconstructed
    pub reconstructed_stripes: Vec<u64>,

    /// Duration of the operation
    pub duration: Duration,
}

// =============================================================================
// Reconstruction Engine
// =============================================================================

/// Engine for handling EC reconstructions
pub struct ReconstructionEngine {
    /// Configuration
    config: ReconstructionConfig,

    /// Metadata manager
    metadata_manager: Arc<EcMetadataManager>,

    /// Active reconstruction tasks
    active_tasks: DashMap<u64, ReconstructionTask>,

    /// Task ID counter
    next_task_id: AtomicU64,

    /// Semaphore for limiting concurrent reconstructions
    semaphore: Arc<Semaphore>,

    /// Shutdown flag
    shutdown: Arc<std::sync::atomic::AtomicBool>,

    /// Task sender
    task_tx: mpsc::Sender<ReconstructionTask>,

    /// Task receiver
    task_rx: Arc<tokio::sync::RwLock<mpsc::Receiver<ReconstructionTask>>>,
}

impl ReconstructionEngine {
    /// Create a new reconstruction engine
    pub fn new(
        config: ReconstructionConfig,
        metadata_manager: Arc<EcMetadataManager>,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::channel(1000);

        Arc::new(Self {
            semaphore: Arc::new(Semaphore::new(config.max_concurrent)),
            config,
            metadata_manager,
            active_tasks: DashMap::new(),
            next_task_id: AtomicU64::new(0),
            shutdown: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            task_tx: tx,
            task_rx: Arc::new(tokio::sync::RwLock::new(rx)),
        })
    }

    /// Handle a degraded read request
    ///
    /// Reads available shards and reconstructs missing ones to serve the read.
    #[instrument(skip(self, request))]
    pub async fn handle_degraded_read(&self, request: ReadRequest) -> Result<ReadResult> {
        let start = std::time::Instant::now();
        let mut reconstructed_stripes = Vec::new();

        // Get volume state
        let volume_state = self
            .metadata_manager
            .get_volume(&request.volume_id)
            .ok_or_else(|| Error::Internal(format!("Volume {} not found", request.volume_id)))?;

        // Get stripes and policy_ref from the locked state in a scope block
        let (stripes, policy_ref) = {
            let state = volume_state.read();
            let stripes = state.find_stripes_in_range(&request.lba_range);
            let policy_ref = state.policy_ref.clone();
            (stripes, policy_ref)
        };

        if stripes.is_empty() {
            return Err(Error::EcStripeNotFound(format!(
                "No stripes found for LBA range {:?}",
                request.lba_range
            )));
        }

        // Load policy for decoder configuration
        let policy = self.metadata_manager.load_policy(&policy_ref).await?;
        let decoder = EcDecoder::new(
            policy.spec.data_shards as usize,
            policy.spec.parity_shards as usize,
        )?;

        let mut result_data = Vec::new();

        for stripe in stripes {
            // Check if stripe is degraded
            let is_degraded = stripe.status.state == StripeState::Degraded;

            if is_degraded && !request.allow_degraded {
                return Err(Error::EcReconstructionFailed {
                    stripe_id: stripe.stripe_id,
                    reason: "Stripe is degraded and degraded reads not allowed".to_string(),
                });
            }

            // Read shards (simulate - in real implementation would read from storage)
            let shards_result = self.read_stripe_shards(&stripe, &decoder).await?;

            if shards_result.needs_reconstruction {
                reconstructed_stripes.push(stripe.stripe_id);

                // Create reconstruction task for background repair
                self.queue_background_rebuild(
                    stripe.volume_id.clone(),
                    stripe.stripe_id,
                    shards_result.missing_indices.clone(),
                )
                .await?;
            }

            // Append data from this stripe
            result_data.extend_from_slice(&shards_result.data);
        }

        // Trim to requested range
        // (In a real implementation, we'd calculate exact offsets)

        Ok(ReadResult {
            data: result_data,
            degraded: !reconstructed_stripes.is_empty(),
            reconstructed_stripes,
            duration: start.elapsed(),
        })
    }

    /// Read shards for a stripe and reconstruct if needed
    async fn read_stripe_shards(
        &self,
        stripe: &StripeMetadata,
        decoder: &EcDecoder,
    ) -> Result<ShardReadResult> {
        let total_shards = decoder.total_shards();
        let mut shards: Vec<Option<Vec<u8>>> = vec![None; total_shards];
        let mut missing_indices = Vec::new();

        // Check shard health from status
        for (i, location) in stripe.shard_locations.iter().enumerate() {
            let shard_healthy = stripe
                .status
                .shard_health
                .get(i)
                .map(|h| h.state == ShardState::Healthy)
                .unwrap_or(true); // Assume healthy if no status

            if shard_healthy {
                // Simulate reading shard (in real implementation, read from storage)
                let shard_data = self.simulate_shard_read(location).await;
                if let Some(data) = shard_data {
                    shards[i] = Some(data);
                } else {
                    missing_indices.push(i as u8);
                }
            } else {
                missing_indices.push(i as u8);
            }
        }

        let needs_reconstruction = !missing_indices.is_empty();
        let available = shards.iter().filter(|s| s.is_some()).count();

        // Check if we have enough shards
        if available < decoder.data_shards() {
            return Err(Error::InsufficientShards {
                available,
                required: decoder.data_shards(),
            });
        }

        // Reconstruct if needed
        if needs_reconstruction {
            debug!(
                "Reconstructing stripe {} with {} missing shards",
                stripe.stripe_id,
                missing_indices.len()
            );
            decoder.reconstruct_data(&mut shards)?;
        }

        // Combine data shards
        let mut data = Vec::new();
        for s in shards.iter().take(decoder.data_shards()).flatten() {
            data.extend_from_slice(s);
        }

        Ok(ShardReadResult {
            data,
            needs_reconstruction,
            missing_indices,
        })
    }

    /// Simulate reading a shard from storage
    async fn simulate_shard_read(&self, _location: &crate::crd::ShardLocation) -> Option<Vec<u8>> {
        // In a real implementation, this would:
        // 1. Connect to the storage node
        // 2. Read the shard data from the specified pool/offset
        // 3. Verify checksum
        // 4. Return the data or None if unavailable

        // For now, return simulated data
        Some(vec![0u8; 256 * 1024]) // 256KB shard
    }

    /// Queue a background rebuild task
    #[instrument(skip(self))]
    pub async fn queue_background_rebuild(
        &self,
        volume_id: String,
        stripe_id: u64,
        missing_shards: Vec<u8>,
    ) -> Result<u64> {
        let task_id = self.next_task_id.fetch_add(1, Ordering::SeqCst);

        let task = ReconstructionTask {
            task_id,
            volume_id,
            stripe_id,
            missing_shards,
            task_type: ReconstructionType::BackgroundRebuild,
            priority: 5, // Medium priority
            created_at: Utc::now(),
            progress: 0,
            status: TaskStatus::Pending,
        };

        self.task_tx
            .send(task.clone())
            .await
            .map_err(|e| Error::Internal(format!("Failed to queue reconstruction task: {}", e)))?;

        self.active_tasks.insert(task_id, task);

        info!(
            "Queued background rebuild for stripe {} (task {})",
            stripe_id, task_id
        );

        Ok(task_id)
    }

    /// Start a stripe reconstruction
    #[instrument(skip(self))]
    pub async fn start_reconstruction(
        &self,
        volume_id: &str,
        stripe_id: u64,
        missing_shards: Vec<u8>,
    ) -> Result<u64> {
        // Check if already reconstructing
        for entry in self.active_tasks.iter() {
            if entry.stripe_id == stripe_id && entry.status == TaskStatus::InProgress {
                return Ok(entry.task_id);
            }
        }

        self.queue_background_rebuild(volume_id.to_string(), stripe_id, missing_shards)
            .await
    }

    /// Run the background rebuild loop
    #[instrument(skip(self))]
    pub async fn run(self: Arc<Self>) {
        info!("Starting reconstruction engine with {:?}", self.config);

        let mut tick = interval(self.config.rebuild_check_interval);

        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if self.shutdown.load(Ordering::Relaxed) {
                        info!("Reconstruction engine shutting down");
                        break;
                    }

                    // Check for degraded stripes that need rebuilding
                    if let Err(e) = self.check_degraded_stripes().await {
                        error!("Error checking degraded stripes: {}", e);
                    }
                }

                // Process queued tasks
                task = async {
                    let mut rx = self.task_rx.write().await;
                    rx.recv().await
                } => {
                    if let Some(task) = task {
                        let engine = Arc::clone(&self);
                        tokio::spawn(async move {
                            if let Err(e) = engine.execute_task(task).await {
                                error!("Reconstruction task failed: {}", e);
                            }
                        });
                    }
                }
            }
        }
    }

    /// Execute a reconstruction task
    #[instrument(skip(self))]
    async fn execute_task(&self, mut task: ReconstructionTask) -> Result<()> {
        // Acquire semaphore permit
        let _permit = self.semaphore.acquire().await.map_err(|e| {
            Error::Internal(format!("Failed to acquire reconstruction permit: {}", e))
        })?;

        // Update task status
        task.status = TaskStatus::InProgress;
        self.active_tasks.insert(task.task_id, task.clone());

        info!(
            "Starting reconstruction task {} for stripe {}",
            task.task_id, task.stripe_id
        );

        // Get volume state
        let volume_state = self
            .metadata_manager
            .get_volume(&task.volume_id)
            .ok_or_else(|| Error::Internal(format!("Volume {} not found", task.volume_id)))?;

        // Extract needed data in a block to ensure guard is dropped before await
        let (stripe, policy_ref) = {
            let state = volume_state.read();
            let stripe = state.get_stripe(task.stripe_id).ok_or_else(|| {
                Error::EcStripeNotFound(format!("Stripe {} not found", task.stripe_id))
            })?;
            let policy_ref = state.policy_ref.clone();
            (stripe, policy_ref)
        };

        // Load policy
        let policy = self.metadata_manager.load_policy(&policy_ref).await?;
        let decoder = EcDecoder::new(
            policy.spec.data_shards as usize,
            policy.spec.parity_shards as usize,
        )?;

        // Read available shards
        let mut shards: Vec<Option<Vec<u8>>> = vec![None; decoder.total_shards()];

        for (i, location) in stripe.shard_locations.iter().enumerate() {
            if !task.missing_shards.contains(&(i as u8)) {
                if let Some(data) = self.simulate_shard_read(location).await {
                    shards[i] = Some(data);
                }
            }
        }

        // Reconstruct missing shards
        decoder.reconstruct(&mut shards)?;

        // Update progress
        task.progress = 50;
        self.active_tasks.insert(task.task_id, task.clone());

        // Write reconstructed shards back to storage
        for &missing_idx in &task.missing_shards {
            if let Some(shard_data) = &shards[missing_idx as usize] {
                // In real implementation, write shard to storage
                debug!(
                    "Would write reconstructed shard {} ({} bytes) for stripe {}",
                    missing_idx,
                    shard_data.len(),
                    task.stripe_id
                );
            }
        }

        // Update stripe status to healthy
        let state = volume_state.read();
        if let Some(mut stripe) = state.get_stripe(task.stripe_id) {
            stripe.status.state = StripeState::Healthy;
            stripe.status.healthy_shards = decoder.total_shards() as u8;
            // In real implementation, update via metadata manager
        }
        drop(state);

        // Mark task complete
        task.status = TaskStatus::Completed;
        task.progress = 100;
        self.active_tasks.insert(task.task_id, task.clone());

        info!(
            "Completed reconstruction task {} for stripe {}",
            task.task_id, task.stripe_id
        );

        // Remove completed task after a delay
        let task_id = task.task_id;
        let tasks = self.active_tasks.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            tasks.remove(&task_id);
        });

        Ok(())
    }

    /// Check all volumes for degraded stripes that need rebuilding
    async fn check_degraded_stripes(&self) -> Result<()> {
        let stats = self.metadata_manager.aggregate_stats();

        if stats.degraded_stripes > 0 {
            debug!(
                "Found {} degraded stripes across {} volumes",
                stats.degraded_stripes, stats.total_volumes
            );
            // In a real implementation, we would iterate through volumes
            // and queue rebuild tasks for degraded stripes
        }

        Ok(())
    }

    /// Signal shutdown
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    /// Get the number of active tasks
    pub fn active_task_count(&self) -> usize {
        self.active_tasks
            .iter()
            .filter(|t| t.status == TaskStatus::InProgress)
            .count()
    }

    /// Get all active tasks
    pub fn get_active_tasks(&self) -> Vec<ReconstructionTask> {
        self.active_tasks.iter().map(|e| e.clone()).collect()
    }

    /// Get task by ID
    pub fn get_task(&self, task_id: u64) -> Option<ReconstructionTask> {
        self.active_tasks.get(&task_id).map(|e| e.clone())
    }

    /// Cancel a task
    pub fn cancel_task(&self, task_id: u64) -> bool {
        if let Some(mut task) = self.active_tasks.get_mut(&task_id) {
            if task.status == TaskStatus::Pending {
                task.status = TaskStatus::Cancelled;
                return true;
            }
        }
        false
    }
}

/// Result of reading shards
struct ShardReadResult {
    data: Vec<u8>,
    needs_reconstruction: bool,
    missing_indices: Vec<u8>,
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
    fn test_reconstruction_config_default() {
        let config = ReconstructionConfig::default();

        assert_eq!(config.max_concurrent, 4);
        assert_eq!(config.shard_read_timeout, Duration::from_secs(30));
        assert_eq!(config.stripe_timeout, Duration::from_secs(300));
        assert_eq!(config.rebuild_check_interval, Duration::from_secs(60));
        assert!(config.prioritize_reads);
    }

    // =========================================================================
    // ReconstructionTask Tests
    // =========================================================================

    #[test]
    fn test_reconstruction_task_creation() {
        let task = ReconstructionTask {
            task_id: 1,
            volume_id: "vol-1".to_string(),
            stripe_id: 42,
            missing_shards: vec![2, 4],
            task_type: ReconstructionType::BackgroundRebuild,
            priority: 5,
            created_at: Utc::now(),
            progress: 0,
            status: TaskStatus::Pending,
        };

        assert_eq!(task.task_id, 1);
        assert_eq!(task.stripe_id, 42);
        assert_eq!(task.missing_shards, vec![2, 4]);
        assert_eq!(task.task_type, ReconstructionType::BackgroundRebuild);
        assert_eq!(task.status, TaskStatus::Pending);
    }

    // =========================================================================
    // ReconstructionType Tests
    // =========================================================================

    #[test]
    fn test_reconstruction_type_equality() {
        assert_eq!(
            ReconstructionType::DegradedRead,
            ReconstructionType::DegradedRead
        );
        assert_ne!(
            ReconstructionType::DegradedRead,
            ReconstructionType::BackgroundRebuild
        );
        assert_ne!(
            ReconstructionType::BackgroundRebuild,
            ReconstructionType::ScrubVerification
        );
    }

    // =========================================================================
    // TaskStatus Tests
    // =========================================================================

    #[test]
    fn test_task_status_equality() {
        assert_eq!(TaskStatus::Pending, TaskStatus::Pending);
        assert_ne!(TaskStatus::Pending, TaskStatus::InProgress);
        assert_ne!(TaskStatus::Completed, TaskStatus::Failed);
    }

    // =========================================================================
    // ReadRequest Tests
    // =========================================================================

    #[test]
    fn test_read_request_creation() {
        let request = ReadRequest {
            volume_id: "vol-1".to_string(),
            lba_range: LbaRange::new(0, 1000),
            allow_degraded: true,
        };

        assert_eq!(request.volume_id, "vol-1");
        assert_eq!(request.lba_range.size(), 1000);
        assert!(request.allow_degraded);
    }

    // =========================================================================
    // ReadResult Tests
    // =========================================================================

    #[test]
    fn test_read_result_creation() {
        let result = ReadResult {
            data: vec![0u8; 1024],
            degraded: true,
            reconstructed_stripes: vec![1, 2, 3],
            duration: Duration::from_millis(100),
        };

        assert_eq!(result.data.len(), 1024);
        assert!(result.degraded);
        assert_eq!(result.reconstructed_stripes, vec![1, 2, 3]);
    }
}
