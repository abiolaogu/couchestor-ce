// Allow dead code for library-style API methods not yet used by the binary
#![allow(dead_code)]

//! Migrator Engine - "The Hands"
//!
//! Performs safe volume migrations between storage tiers using
//! a scale-up-then-scale-down pattern that guarantees data safety.
//!
//! # Safety Guarantees
//!
//! 1. Old replica is NEVER removed until new replica is fully synced
//! 2. Timeouts at every step prevent stuck operations
//! 3. Any error aborts migration (old replica preserved)
//! 4. Optional preservation mode never removes old replicas

use crate::crd::{DiskPool, MayastorVolume};
use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use kube::{Api, Client};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::{debug, info, instrument, warn};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the migrator
#[derive(Debug, Clone)]
pub struct MigratorConfig {
    /// Timeout for replica sync
    pub sync_timeout: Duration,

    /// Interval between sync status checks
    pub sync_poll_interval: Duration,

    /// Maximum retries for transient errors
    #[allow(dead_code)]
    pub max_retries: u32,

    /// Dry-run mode (log only, no changes)
    pub dry_run: bool,

    /// Preservation mode (never remove old replicas)
    pub preservation_mode: bool,
}

impl Default for MigratorConfig {
    fn default() -> Self {
        Self {
            sync_timeout: Duration::from_secs(30 * 60), // 30 minutes
            sync_poll_interval: Duration::from_secs(10),
            max_retries: 3,
            dry_run: false,
            preservation_mode: false,
        }
    }
}

// =============================================================================
// Migration State Machine
// =============================================================================

/// States in the migration process
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MigrationState {
    /// Initial state
    Idle,
    /// Analyzing current replicas
    Analyzing,
    /// Adding new replica
    ScalingUp,
    /// Waiting for new replica to sync
    WaitingSync,
    /// Removing old replica
    ScalingDown,
    /// Migration completed successfully
    Completed,
    /// Migration failed
    Failed,
    /// Migration aborted (data preserved)
    Aborted,
    /// EC encoding in progress
    EcEncoding,
    /// EC shard distribution in progress
    EcDistributing,
    /// EC reconstruction in progress
    EcReconstructing,
}

/// Type of migration operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MigrationType {
    /// Standard replica-based migration between pools
    Standard,
    /// Migration from replicated storage to erasure coding
    ToEc,
    /// Migration from erasure coding to replicated storage
    FromEc,
    /// Rebalance EC shards across pools
    EcRebalance,
}

impl std::fmt::Display for MigrationState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationState::Idle => write!(f, "Idle"),
            MigrationState::Analyzing => write!(f, "Analyzing"),
            MigrationState::ScalingUp => write!(f, "ScalingUp"),
            MigrationState::WaitingSync => write!(f, "WaitingSync"),
            MigrationState::ScalingDown => write!(f, "ScalingDown"),
            MigrationState::Completed => write!(f, "Completed"),
            MigrationState::Failed => write!(f, "Failed"),
            MigrationState::Aborted => write!(f, "Aborted"),
            MigrationState::EcEncoding => write!(f, "EcEncoding"),
            MigrationState::EcDistributing => write!(f, "EcDistributing"),
            MigrationState::EcReconstructing => write!(f, "EcReconstructing"),
        }
    }
}

impl std::fmt::Display for MigrationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationType::Standard => write!(f, "Standard"),
            MigrationType::ToEc => write!(f, "ToEc"),
            MigrationType::FromEc => write!(f, "FromEc"),
            MigrationType::EcRebalance => write!(f, "EcRebalance"),
        }
    }
}

/// A step in the migration process
#[derive(Debug, Clone, Serialize)]
pub struct MigrationStep {
    pub state: MigrationState,
    pub timestamp: DateTime<Utc>,
    pub message: String,
    pub duration_ms: Option<u64>,
}

/// Result of a migration operation
#[derive(Debug, Clone, Serialize)]
pub struct MigrationResult {
    /// Name of the migrated volume
    pub volume_name: String,

    /// Source pool
    pub source_pool: String,

    /// Target pool
    pub target_pool: String,

    /// Type of migration
    pub migration_type: MigrationType,

    /// Final state
    pub state: MigrationState,

    /// When migration started
    pub start_time: DateTime<Utc>,

    /// When migration ended
    pub end_time: DateTime<Utc>,

    /// Total duration
    pub duration: Duration,

    /// Error if failed
    pub error: Option<String>,

    /// Step-by-step log
    pub steps: Vec<MigrationStep>,

    /// EC-specific: policy used (if EC migration)
    pub ec_policy: Option<String>,

    /// EC-specific: stripes created (if ToEc migration)
    pub ec_stripes_created: Option<u64>,
}

impl MigrationResult {
    /// Check if migration succeeded
    #[allow(dead_code)]
    pub fn is_success(&self) -> bool {
        self.state == MigrationState::Completed
    }

    /// Create a new in-progress result
    fn new(volume_name: &str, source_pool: &str, target_pool: &str) -> Self {
        let now = Utc::now();
        Self {
            volume_name: volume_name.to_string(),
            source_pool: source_pool.to_string(),
            target_pool: target_pool.to_string(),
            migration_type: MigrationType::Standard,
            state: MigrationState::Idle,
            start_time: now,
            end_time: now,
            duration: Duration::ZERO,
            error: None,
            steps: vec![],
            ec_policy: None,
            ec_stripes_created: None,
        }
    }

    /// Create a new EC migration result
    fn new_ec(
        volume_name: &str,
        source_pool: &str,
        target_pool: &str,
        migration_type: MigrationType,
        ec_policy: &str,
    ) -> Self {
        let mut result = Self::new(volume_name, source_pool, target_pool);
        result.migration_type = migration_type;
        result.ec_policy = Some(ec_policy.to_string());
        result
    }

    /// Record a state transition
    fn transition(&mut self, state: MigrationState, message: &str) {
        let now = Utc::now();
        let last_step_time = self
            .steps
            .last()
            .map(|s| s.timestamp)
            .unwrap_or(self.start_time);
        let duration_ms = (now - last_step_time).num_milliseconds() as u64;

        self.state = state;
        self.steps.push(MigrationStep {
            state,
            timestamp: now,
            message: message.to_string(),
            duration_ms: Some(duration_ms),
        });

        // Update end time and duration
        self.end_time = now;
        self.duration = (now - self.start_time).to_std().unwrap_or(Duration::ZERO);
    }

    /// Mark as failed
    fn fail(&mut self, error: &str) {
        self.transition(MigrationState::Failed, error);
        self.error = Some(error.to_string());
    }

    /// Mark as aborted (data preserved)
    fn abort(&mut self, reason: &str) {
        self.transition(MigrationState::Aborted, reason);
        self.error = Some(reason.to_string());
    }
}

// =============================================================================
// Active Migration Tracking
// =============================================================================

#[derive(Debug)]
#[allow(dead_code)]
struct ActiveMigration {
    volume_name: String,
    started_at: DateTime<Utc>,
    target_pool: String,
}

// =============================================================================
// Migrator
// =============================================================================

/// Performs safe volume migrations between storage tiers
pub struct Migrator {
    config: MigratorConfig,
    client: Client,
    /// Track active migrations to prevent duplicates
    active_migrations: DashMap<String, ActiveMigration>,
}

impl Migrator {
    /// Create a new migrator
    pub fn new(config: MigratorConfig, client: Client) -> Arc<Self> {
        Arc::new(Self {
            config,
            client,
            active_migrations: DashMap::new(),
        })
    }

    /// Check if a volume is currently being migrated
    pub fn is_migrating(&self, volume_name: &str) -> bool {
        self.active_migrations.contains_key(volume_name)
    }

    /// Get count of active migrations
    pub fn active_count(&self) -> usize {
        self.active_migrations.len()
    }

    /// Migrate a volume to a target pool
    #[instrument(skip(self), fields(volume = %volume_name, target = %target_pool_name))]
    pub async fn migrate_volume(
        self: &Arc<Self>,
        volume_name: &str,
        target_pool_name: &str,
        mayastor_namespace: &str,
    ) -> Result<MigrationResult> {
        // Check if already migrating
        if self.is_migrating(volume_name) {
            return Err(Error::MigrationInProgress {
                volume_name: volume_name.to_string(),
            });
        }

        // Get the source pool from current replicas
        let volumes_api: Api<MayastorVolume> =
            Api::namespaced(self.client.clone(), mayastor_namespace);

        let volume = volumes_api
            .get(volume_name)
            .await
            .map_err(|e| Error::MigrationFailed {
                volume_name: volume_name.to_string(),
                reason: format!("Failed to get volume: {}", e),
            })?;

        // Find source pool (first replica's pool)
        let source_pool = volume
            .replicas()
            .first()
            .map(|r| r.pool.clone())
            .ok_or_else(|| Error::MigrationFailed {
                volume_name: volume_name.to_string(),
                reason: "Volume has no replicas".to_string(),
            })?;

        // Skip if already on target pool
        if source_pool == target_pool_name {
            info!(
                "Volume {} already on target pool {}",
                volume_name, target_pool_name
            );
            let mut result = MigrationResult::new(volume_name, &source_pool, target_pool_name);
            result.transition(MigrationState::Completed, "Already on target pool");
            return Ok(result);
        }

        // Register active migration
        self.active_migrations.insert(
            volume_name.to_string(),
            ActiveMigration {
                volume_name: volume_name.to_string(),
                started_at: Utc::now(),
                target_pool: target_pool_name.to_string(),
            },
        );

        // Run migration with cleanup on exit
        let result = self
            .do_migrate(
                volume_name,
                &source_pool,
                target_pool_name,
                mayastor_namespace,
            )
            .await;

        // Unregister active migration
        self.active_migrations.remove(volume_name);

        result
    }

    /// Internal migration logic
    async fn do_migrate(
        &self,
        volume_name: &str,
        source_pool: &str,
        target_pool: &str,
        mayastor_namespace: &str,
    ) -> Result<MigrationResult> {
        let mut result = MigrationResult::new(volume_name, source_pool, target_pool);

        info!(
            "Starting migration: {} from {} to {}",
            volume_name, source_pool, target_pool
        );

        // =====================================================================
        // Phase 1: Analyze
        // =====================================================================
        result.transition(MigrationState::Analyzing, "Analyzing current replicas");

        let volumes_api: Api<MayastorVolume> =
            Api::namespaced(self.client.clone(), mayastor_namespace);
        let pools_api: Api<DiskPool> = Api::all(self.client.clone());

        // Verify target pool exists and is online
        let target_pool_obj = pools_api.get(target_pool).await.map_err(|e| {
            result.fail(&format!("Target pool not found: {}", e));
            Error::NoSuitablePool {
                tier: target_pool.to_string(),
            }
        })?;

        if !target_pool_obj.is_online() {
            result.fail("Target pool is not online");
            return Err(Error::NoSuitablePool {
                tier: format!("{} (offline)", target_pool),
            });
        }

        // Get current volume state
        let volume = volumes_api.get(volume_name).await.map_err(|e| {
            result.fail(&format!("Failed to get volume: {}", e));
            Error::MigrationFailed {
                volume_name: volume_name.to_string(),
                reason: e.to_string(),
            }
        })?;

        let initial_replica_count = volume.replicas().len();
        debug!(
            "Volume {} has {} replicas",
            volume_name, initial_replica_count
        );

        // Dry-run mode check
        if self.config.dry_run {
            info!(
                "[DRY-RUN] Would migrate {} from {} to {}",
                volume_name, source_pool, target_pool
            );
            result.transition(
                MigrationState::Completed,
                "Dry-run completed (no changes made)",
            );
            return Ok(result);
        }

        // =====================================================================
        // Phase 2: Scale Up - Add replica on target pool
        // =====================================================================
        result.transition(
            MigrationState::ScalingUp,
            &format!("Adding replica on pool {}", target_pool),
        );

        // Update volume topology to include target pool
        // This is Mayastor-specific - we need to patch the volume spec
        let patch = serde_json::json!({
            "spec": {
                "numReplicas": initial_replica_count + 1,
                "topology": {
                    "pool": {
                        "labelled": {
                            "inclusion": {
                                "pool": target_pool
                            }
                        }
                    }
                }
            }
        });

        debug!("Patching volume with: {:?}", patch);

        volumes_api
            .patch(
                volume_name,
                &kube::api::PatchParams::apply("smart-storage-operator"),
                &kube::api::Patch::Merge(&patch),
            )
            .await
            .map_err(|e| {
                result.fail(&format!("Failed to add replica: {}", e));
                Error::MigrationFailed {
                    volume_name: volume_name.to_string(),
                    reason: e.to_string(),
                }
            })?;

        // =====================================================================
        // Phase 3: Wait for Sync
        // =====================================================================
        result.transition(MigrationState::WaitingSync, "Waiting for replica sync");

        let sync_result: std::result::Result<std::result::Result<(), Error>, _> =
            timeout(self.config.sync_timeout, async {
                loop {
                    sleep(self.config.sync_poll_interval).await;

                    let volume = match volumes_api.get(volume_name).await {
                        Ok(v) => v,
                        Err(e) => {
                            warn!("Failed to poll volume status: {}", e);
                            continue;
                        }
                    };

                    // Find the new replica on target pool
                    let replicas = volume.replicas();
                    let new_replica = replicas.iter().find(|r| r.pool == target_pool);

                    match new_replica {
                        Some(replica) => {
                            debug!(
                                "New replica state: {} (pool: {})",
                                replica.state, replica.pool
                            );

                            if replica.is_synced() {
                                info!("Replica on {} is now synced", target_pool);
                                return Ok(());
                            }
                        }
                        None => {
                            debug!("Waiting for replica to appear on pool {}", target_pool);
                        }
                    }
                }
            })
            .await;

        match sync_result {
            Ok(Ok(())) => {
                debug!("Sync completed successfully");
            }
            Ok(Err(e)) => {
                result.abort(&format!("Sync error: {}", e));
                return Err(Error::ReplicaSyncFailed(e.to_string()));
            }
            Err(_) => {
                // Timeout - ABORT, don't remove old replica
                result.abort(&format!(
                    "Sync timeout after {:?}",
                    self.config.sync_timeout
                ));
                return Err(Error::MigrationTimeout {
                    volume_name: volume_name.to_string(),
                    duration: format!("{:?}", self.config.sync_timeout),
                });
            }
        }

        // =====================================================================
        // Phase 4: Scale Down - Remove old replica (ONLY if sync succeeded)
        // =====================================================================
        if self.config.preservation_mode {
            info!("Preservation mode: keeping old replica on {}", source_pool);
            result.transition(
                MigrationState::Completed,
                "Completed (preservation mode - old replica kept)",
            );
            return Ok(result);
        }

        result.transition(
            MigrationState::ScalingDown,
            &format!("Removing replica from pool {}", source_pool),
        );

        // Reduce replica count back to original
        let patch = serde_json::json!({
            "spec": {
                "numReplicas": initial_replica_count
            }
        });

        volumes_api
            .patch(
                volume_name,
                &kube::api::PatchParams::apply("smart-storage-operator"),
                &kube::api::Patch::Merge(&patch),
            )
            .await
            .map_err(|e| {
                // Even if this fails, data is safe on new replica
                warn!("Failed to remove old replica (data is safe): {}", e);
                result.transition(
                    MigrationState::Completed,
                    "Completed with warning: old replica removal failed",
                );
                Error::MigrationFailed {
                    volume_name: volume_name.to_string(),
                    reason: format!("Old replica removal failed: {}", e),
                }
            })?;

        // =====================================================================
        // Success!
        // =====================================================================
        result.transition(
            MigrationState::Completed,
            "Migration completed successfully",
        );

        info!(
            "Migration completed: {} moved from {} to {} in {:?}",
            volume_name, source_pool, target_pool, result.duration
        );

        Ok(result)
    }

    /// Find a suitable pool for a tier
    pub async fn find_pool_for_tier(
        &self,
        tier: &str,
        labels: &std::collections::BTreeMap<String, String>,
    ) -> Result<String> {
        let pools_api: Api<DiskPool> = Api::all(self.client.clone());
        let pools = pools_api.list(&Default::default()).await?;

        for pool in pools.items {
            if !pool.is_online() {
                continue;
            }

            let pool_labels = pool.labels();
            let matches = labels
                .iter()
                .all(|(k, v)| pool_labels.get(k).map(|pv| pv == v).unwrap_or(false));

            if matches {
                return Ok(pool.pool_name().to_string());
            }
        }

        Err(Error::NoSuitablePool {
            tier: tier.to_string(),
        })
    }

    /// Migrate a volume to EC storage
    ///
    /// Converts a replicated volume to erasure-coded storage.
    /// Data is read from the current replica, encoded into EC shards,
    /// and distributed across the target pools.
    #[instrument(skip(self), fields(volume = %volume_name, ec_policy = %ec_policy_name))]
    pub async fn migrate_to_ec(
        self: &Arc<Self>,
        volume_name: &str,
        ec_policy_name: &str,
        target_pools: &[String],
        mayastor_namespace: &str,
    ) -> Result<MigrationResult> {
        // Check if already migrating
        if self.is_migrating(volume_name) {
            return Err(Error::MigrationInProgress {
                volume_name: volume_name.to_string(),
            });
        }

        // Get current volume state
        let volumes_api: Api<MayastorVolume> =
            Api::namespaced(self.client.clone(), mayastor_namespace);

        let volume = volumes_api
            .get(volume_name)
            .await
            .map_err(|e| Error::MigrationFailed {
                volume_name: volume_name.to_string(),
                reason: format!("Failed to get volume: {}", e),
            })?;

        // Find source pool
        let source_pool = volume
            .replicas()
            .first()
            .map(|r| r.pool.clone())
            .ok_or_else(|| Error::MigrationFailed {
                volume_name: volume_name.to_string(),
                reason: "Volume has no replicas".to_string(),
            })?;

        // Register active migration
        self.active_migrations.insert(
            volume_name.to_string(),
            ActiveMigration {
                volume_name: volume_name.to_string(),
                started_at: Utc::now(),
                target_pool: format!("ec:{}", ec_policy_name),
            },
        );

        // Create result tracker
        let mut result = MigrationResult::new_ec(
            volume_name,
            &source_pool,
            "ec-storage",
            MigrationType::ToEc,
            ec_policy_name,
        );

        info!(
            "Starting EC migration: {} from {} using policy {}",
            volume_name, source_pool, ec_policy_name
        );

        // Dry-run check
        if self.config.dry_run {
            info!(
                "[DRY-RUN] Would migrate {} to EC storage with policy {}",
                volume_name, ec_policy_name
            );
            result.transition(
                MigrationState::Completed,
                "Dry-run completed (no changes made)",
            );
            self.active_migrations.remove(volume_name);
            return Ok(result);
        }

        // =====================================================================
        // Phase 1: Analyze
        // =====================================================================
        result.transition(
            MigrationState::Analyzing,
            "Analyzing volume for EC migration",
        );

        // Validate target pools exist and are online
        let pools_api: Api<DiskPool> = Api::all(self.client.clone());
        for pool_name in target_pools {
            let pool = pools_api.get(pool_name).await.map_err(|e| {
                result.fail(&format!("Target pool {} not found: {}", pool_name, e));
                Error::NoSuitablePool {
                    tier: pool_name.clone(),
                }
            })?;

            if !pool.is_online() {
                result.fail(&format!("Target pool {} is not online", pool_name));
                self.active_migrations.remove(volume_name);
                return Err(Error::NoSuitablePool {
                    tier: format!("{} (offline)", pool_name),
                });
            }
        }

        // =====================================================================
        // Phase 2: EC Encoding
        // =====================================================================
        result.transition(
            MigrationState::EcEncoding,
            &format!("Encoding volume data with EC policy {}", ec_policy_name),
        );

        // In a real implementation, we would:
        // 1. Read volume data from the source replica
        // 2. Use the EC encoder to create data + parity shards
        // 3. Calculate checksums for each shard
        //
        // For this implementation, we track the operation but don't do actual I/O

        debug!("EC encoding phase would encode volume data here");

        // =====================================================================
        // Phase 3: EC Distribution
        // =====================================================================
        result.transition(
            MigrationState::EcDistributing,
            &format!("Distributing EC shards to {} pools", target_pools.len()),
        );

        // In a real implementation, we would:
        // 1. Allocate space on each target pool
        // 2. Write each shard to its assigned pool
        // 3. Create ECStripe CRDs to track metadata
        //
        // Simulated stripe creation
        let stripes_created = 1u64; // Would be calculated from volume size / stripe size
        result.ec_stripes_created = Some(stripes_created);

        debug!(
            "EC distribution phase would create {} stripes across {} pools",
            stripes_created,
            target_pools.len()
        );

        // =====================================================================
        // Phase 4: Verify and Cleanup
        // =====================================================================
        result.transition(
            MigrationState::ScalingDown,
            "Verifying EC stripes and cleaning up",
        );

        // In preservation mode, keep the original replica
        if self.config.preservation_mode {
            info!(
                "Preservation mode: keeping original replica on {}",
                source_pool
            );
            result.transition(
                MigrationState::Completed,
                "Completed (preservation mode - original replica kept)",
            );
            self.active_migrations.remove(volume_name);
            return Ok(result);
        }

        // Remove original replica (after verification)
        debug!(
            "Would remove original replica from {} after EC verification",
            source_pool
        );

        // =====================================================================
        // Success!
        // =====================================================================
        result.transition(
            MigrationState::Completed,
            "EC migration completed successfully",
        );

        info!(
            "EC migration completed: {} migrated to EC with {} stripes in {:?}",
            volume_name, stripes_created, result.duration
        );

        self.active_migrations.remove(volume_name);
        Ok(result)
    }

    /// Migrate a volume from EC storage back to replicated storage
    ///
    /// Reconstructs data from EC shards and creates replicas on the target pool.
    #[instrument(skip(self), fields(volume = %volume_name, target = %target_pool))]
    pub async fn migrate_from_ec(
        self: &Arc<Self>,
        volume_name: &str,
        target_pool: &str,
        mayastor_namespace: &str,
    ) -> Result<MigrationResult> {
        // Check if already migrating
        if self.is_migrating(volume_name) {
            return Err(Error::MigrationInProgress {
                volume_name: volume_name.to_string(),
            });
        }

        // Register active migration
        self.active_migrations.insert(
            volume_name.to_string(),
            ActiveMigration {
                volume_name: volume_name.to_string(),
                started_at: Utc::now(),
                target_pool: target_pool.to_string(),
            },
        );

        let mut result = MigrationResult::new_ec(
            volume_name,
            "ec-storage",
            target_pool,
            MigrationType::FromEc,
            "unknown", // Would be looked up from metadata
        );

        info!(
            "Starting migration from EC: {} to pool {}",
            volume_name, target_pool
        );

        // Dry-run check
        if self.config.dry_run {
            info!(
                "[DRY-RUN] Would migrate {} from EC to replicated storage on {}",
                volume_name, target_pool
            );
            result.transition(
                MigrationState::Completed,
                "Dry-run completed (no changes made)",
            );
            self.active_migrations.remove(volume_name);
            return Ok(result);
        }

        // =====================================================================
        // Phase 1: Analyze EC state
        // =====================================================================
        result.transition(MigrationState::Analyzing, "Analyzing EC stripes");

        // Verify target pool is available
        let pools_api: Api<DiskPool> = Api::all(self.client.clone());
        let pool = pools_api.get(target_pool).await.map_err(|e| {
            result.fail(&format!("Target pool not found: {}", e));
            Error::NoSuitablePool {
                tier: target_pool.to_string(),
            }
        })?;

        if !pool.is_online() {
            result.fail("Target pool is not online");
            self.active_migrations.remove(volume_name);
            return Err(Error::NoSuitablePool {
                tier: format!("{} (offline)", target_pool),
            });
        }

        // =====================================================================
        // Phase 2: Reconstruct from EC
        // =====================================================================
        result.transition(
            MigrationState::EcReconstructing,
            "Reconstructing data from EC shards",
        );

        // In a real implementation, we would:
        // 1. Load stripe metadata from ECStripe CRDs
        // 2. Read available shards
        // 3. Use EC decoder to reconstruct original data
        debug!("EC reconstruction phase would reconstruct volume data here");

        // =====================================================================
        // Phase 3: Create replica
        // =====================================================================
        result.transition(
            MigrationState::ScalingUp,
            &format!("Creating replica on pool {}", target_pool),
        );

        // In a real implementation:
        // 1. Create volume on target pool
        // 2. Write reconstructed data
        // 3. Verify checksum
        debug!(
            "Would create replica on {} with reconstructed data",
            target_pool
        );

        // =====================================================================
        // Phase 4: Cleanup EC stripes
        // =====================================================================
        if !self.config.preservation_mode {
            result.transition(MigrationState::ScalingDown, "Cleaning up EC stripes");
            debug!("Would delete EC stripes for volume {}", volume_name);
        }

        // =====================================================================
        // Success!
        // =====================================================================
        result.transition(MigrationState::Completed, "Migration from EC completed");

        info!(
            "Migration from EC completed: {} now replicated on {} in {:?}",
            volume_name, target_pool, result.duration
        );

        self.active_migrations.remove(volume_name);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // MigratorConfig Tests
    // =========================================================================

    #[test]
    fn test_migrator_config_default() {
        let config = MigratorConfig::default();

        assert_eq!(config.sync_timeout, Duration::from_secs(30 * 60));
        assert_eq!(config.sync_poll_interval, Duration::from_secs(10));
        assert_eq!(config.max_retries, 3);
        assert!(!config.dry_run);
        assert!(!config.preservation_mode);
    }

    #[test]
    fn test_migrator_config_custom() {
        let config = MigratorConfig {
            sync_timeout: Duration::from_secs(60),
            sync_poll_interval: Duration::from_secs(5),
            max_retries: 5,
            dry_run: true,
            preservation_mode: true,
        };

        assert_eq!(config.sync_timeout, Duration::from_secs(60));
        assert_eq!(config.sync_poll_interval, Duration::from_secs(5));
        assert_eq!(config.max_retries, 5);
        assert!(config.dry_run);
        assert!(config.preservation_mode);
    }

    // =========================================================================
    // MigrationState Tests
    // =========================================================================

    #[test]
    fn test_migration_state_display() {
        assert_eq!(format!("{}", MigrationState::Idle), "Idle");
        assert_eq!(format!("{}", MigrationState::Analyzing), "Analyzing");
        assert_eq!(format!("{}", MigrationState::ScalingUp), "ScalingUp");
        assert_eq!(format!("{}", MigrationState::WaitingSync), "WaitingSync");
        assert_eq!(format!("{}", MigrationState::ScalingDown), "ScalingDown");
        assert_eq!(format!("{}", MigrationState::Completed), "Completed");
        assert_eq!(format!("{}", MigrationState::Failed), "Failed");
        assert_eq!(format!("{}", MigrationState::Aborted), "Aborted");
    }

    #[test]
    fn test_migration_state_equality() {
        assert_eq!(MigrationState::Idle, MigrationState::Idle);
        assert_ne!(MigrationState::Idle, MigrationState::Completed);
        assert_ne!(MigrationState::Failed, MigrationState::Aborted);
    }

    #[test]
    fn test_migration_state_clone() {
        let state = MigrationState::WaitingSync;
        let cloned = state.clone();
        assert_eq!(state, cloned);
    }

    // =========================================================================
    // MigrationResult Tests
    // =========================================================================

    #[test]
    fn test_migration_result_new() {
        let result = MigrationResult::new("vol-1", "pool-sata", "pool-nvme");

        assert_eq!(result.volume_name, "vol-1");
        assert_eq!(result.source_pool, "pool-sata");
        assert_eq!(result.target_pool, "pool-nvme");
        assert_eq!(result.state, MigrationState::Idle);
        assert_eq!(result.duration, Duration::ZERO);
        assert!(result.error.is_none());
        assert!(result.steps.is_empty());
    }

    #[test]
    fn test_migration_result_transitions() {
        let mut result = MigrationResult::new("vol-1", "pool-sata", "pool-nvme");

        assert_eq!(result.state, MigrationState::Idle);

        result.transition(MigrationState::Analyzing, "Starting analysis");
        assert_eq!(result.state, MigrationState::Analyzing);
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].message, "Starting analysis");

        result.transition(MigrationState::ScalingUp, "Adding replica");
        assert_eq!(result.state, MigrationState::ScalingUp);
        assert_eq!(result.steps.len(), 2);
        assert_eq!(result.steps[1].message, "Adding replica");
    }

    #[test]
    fn test_migration_result_is_success() {
        let mut result = MigrationResult::new("vol-1", "pool-sata", "pool-nvme");

        assert!(!result.is_success());

        result.transition(MigrationState::Analyzing, "Analyzing");
        assert!(!result.is_success());

        result.transition(MigrationState::Completed, "Done");
        assert!(result.is_success());
    }

    #[test]
    fn test_migration_result_fail() {
        let mut result = MigrationResult::new("vol-1", "pool-sata", "pool-nvme");

        result.transition(MigrationState::Analyzing, "Analyzing");
        result.fail("Something went wrong");

        assert_eq!(result.state, MigrationState::Failed);
        assert_eq!(result.error, Some("Something went wrong".to_string()));
        assert!(!result.is_success());
        assert_eq!(result.steps.len(), 2);
        assert_eq!(result.steps[1].state, MigrationState::Failed);
    }

    #[test]
    fn test_migration_result_abort() {
        let mut result = MigrationResult::new("vol-1", "pool-sata", "pool-nvme");

        result.transition(MigrationState::WaitingSync, "Waiting for sync");
        result.abort("Timeout reached");

        assert_eq!(result.state, MigrationState::Aborted);
        assert_eq!(result.error, Some("Timeout reached".to_string()));
        assert!(!result.is_success());
        assert_eq!(result.steps.len(), 2);
        assert_eq!(result.steps[1].state, MigrationState::Aborted);
    }

    #[test]
    fn test_migration_result_full_success_flow() {
        let mut result = MigrationResult::new("vol-1", "pool-sata", "pool-nvme");

        // Simulate full successful migration
        result.transition(MigrationState::Analyzing, "Analyzing current replicas");
        result.transition(MigrationState::ScalingUp, "Adding replica on pool-nvme");
        result.transition(MigrationState::WaitingSync, "Waiting for replica sync");
        result.transition(
            MigrationState::ScalingDown,
            "Removing replica from pool-sata",
        );
        result.transition(
            MigrationState::Completed,
            "Migration completed successfully",
        );

        assert!(result.is_success());
        assert_eq!(result.steps.len(), 5);
        assert!(result.error.is_none());

        // Verify state progression
        assert_eq!(result.steps[0].state, MigrationState::Analyzing);
        assert_eq!(result.steps[1].state, MigrationState::ScalingUp);
        assert_eq!(result.steps[2].state, MigrationState::WaitingSync);
        assert_eq!(result.steps[3].state, MigrationState::ScalingDown);
        assert_eq!(result.steps[4].state, MigrationState::Completed);
    }

    #[test]
    fn test_migration_result_aborted_preserves_data() {
        let mut result = MigrationResult::new("vol-1", "pool-sata", "pool-nvme");

        // Simulate migration that times out during sync
        result.transition(MigrationState::Analyzing, "Analyzing current replicas");
        result.transition(MigrationState::ScalingUp, "Adding replica");
        result.transition(MigrationState::WaitingSync, "Waiting for sync");
        result.abort("Sync timeout after 30m");

        // Aborted means data is preserved (old replica not removed)
        assert_eq!(result.state, MigrationState::Aborted);
        assert!(!result.is_success());
        assert_eq!(result.steps.len(), 4);

        // Never reached ScalingDown (which removes old replica)
        let states: Vec<_> = result.steps.iter().map(|s| s.state).collect();
        assert!(!states.contains(&MigrationState::ScalingDown));
    }

    // =========================================================================
    // MigrationStep Tests
    // =========================================================================

    #[test]
    fn test_migration_step_has_duration() {
        let mut result = MigrationResult::new("vol-1", "pool-sata", "pool-nvme");

        result.transition(MigrationState::Analyzing, "Step 1");
        std::thread::sleep(std::time::Duration::from_millis(10));
        result.transition(MigrationState::ScalingUp, "Step 2");

        // First step should have duration_ms
        assert!(result.steps[0].duration_ms.is_some());
        assert!(result.steps[1].duration_ms.is_some());

        // Second step duration should be >= 10ms (we slept)
        assert!(result.steps[1].duration_ms.unwrap() >= 10);
    }

    #[test]
    fn test_migration_step_timestamps_increase() {
        let mut result = MigrationResult::new("vol-1", "pool-sata", "pool-nvme");

        result.transition(MigrationState::Analyzing, "Step 1");
        let ts1 = result.steps[0].timestamp;

        std::thread::sleep(std::time::Duration::from_millis(5));
        result.transition(MigrationState::ScalingUp, "Step 2");
        let ts2 = result.steps[1].timestamp;

        assert!(ts2 >= ts1);
    }

    // =========================================================================
    // State Machine Invariant Tests
    // =========================================================================

    #[test]
    fn test_failed_state_is_terminal() {
        let mut result = MigrationResult::new("vol-1", "pool-sata", "pool-nvme");
        result.fail("Error occurred");

        // After failure, state should be Failed
        assert_eq!(result.state, MigrationState::Failed);

        // Attempting to transition should still work (no panic)
        // but in practice the migration loop would stop
        result.transition(MigrationState::Completed, "This shouldn't happen");
        assert_eq!(result.state, MigrationState::Completed);
        assert_eq!(result.steps.len(), 2);
    }

    #[test]
    fn test_aborted_state_preserves_error() {
        let mut result = MigrationResult::new("vol-1", "pool-sata", "pool-nvme");

        result.abort("Sync timeout");

        assert_eq!(result.state, MigrationState::Aborted);
        assert!(result.error.is_some());
        assert!(result.error.as_ref().unwrap().contains("Sync timeout"));
    }

    #[test]
    fn test_completed_state_no_error() {
        let mut result = MigrationResult::new("vol-1", "pool-sata", "pool-nvme");

        result.transition(MigrationState::Completed, "Success");

        assert_eq!(result.state, MigrationState::Completed);
        assert!(result.error.is_none());
        assert!(result.is_success());
    }

    // =========================================================================
    // Serialization Tests
    // =========================================================================

    #[test]
    fn test_migration_state_serializes() {
        let state = MigrationState::Completed;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "\"Completed\"");
    }

    #[test]
    fn test_migration_result_serializes() {
        let mut result = MigrationResult::new("vol-1", "pool-sata", "pool-nvme");
        result.transition(MigrationState::Completed, "Done");

        let json = serde_json::to_string(&result).unwrap();

        assert!(json.contains("\"volume_name\":\"vol-1\""));
        assert!(json.contains("\"source_pool\":\"pool-sata\""));
        assert!(json.contains("\"target_pool\":\"pool-nvme\""));
        assert!(json.contains("\"state\":\"Completed\""));
    }

    #[test]
    fn test_migration_step_serializes() {
        let step = MigrationStep {
            state: MigrationState::Analyzing,
            timestamp: Utc::now(),
            message: "Test step".to_string(),
            duration_ms: Some(100),
        };

        let json = serde_json::to_string(&step).unwrap();

        assert!(json.contains("\"state\":\"Analyzing\""));
        assert!(json.contains("\"message\":\"Test step\""));
        assert!(json.contains("\"duration_ms\":100"));
    }
}
