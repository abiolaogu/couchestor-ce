//! StoragePolicy Controller - "The Brain"
//!
//! Reconciles StoragePolicy resources by:
//! 1. Watching managed volumes
//! 2. Querying their IOPS metrics
//! 3. Deciding which volumes need migration
//! 4. Triggering migrations safely

use crate::crd::{
    parse_duration, ConditionStatus, DiskPool, LabelSelector, MigrationHistoryEntry,
    PolicyCondition, PolicyPhase, StoragePolicy, StoragePolicyStatus,
};
use crate::error::{Error, Result};
use crate::metrics::{HeatScore, MetricsWatcher};
use crate::migrator::{MigrationResult, Migrator};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use futures::StreamExt;
use k8s_openapi::api::core::v1::PersistentVolume;
use kube::{
    api::{Api, ListParams, Patch, PatchParams},
    runtime::{
        controller::{Action, Controller},
        watcher::Config,
    },
    Client, ResourceExt,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, instrument, warn};

// =============================================================================
// Constants
// =============================================================================

/// Annotation for last migration timestamp
const ANNOTATION_LAST_MIGRATION: &str = "storage.billyronks.io/last-migration";

/// Annotation for current tier
const ANNOTATION_CURRENT_TIER: &str = "storage.billyronks.io/current-tier";

/// Annotation for managing policy
const ANNOTATION_MANAGED_BY: &str = "storage.billyronks.io/managed-by";

/// Default requeue interval
const DEFAULT_REQUEUE_INTERVAL: Duration = Duration::from_secs(300); // 5 minutes

/// Mayastor namespace (default)
const MAYASTOR_NAMESPACE: &str = "mayastor";

// =============================================================================
// Controller Context
// =============================================================================

/// Shared context for the controller
pub struct ControllerContext {
    /// Kubernetes client
    pub client: Client,

    /// Metrics watcher
    pub metrics: Arc<MetricsWatcher>,

    /// Migrator
    pub migrator: Arc<Migrator>,

    /// Semaphore for rate-limiting concurrent migrations
    pub migration_semaphore: Semaphore,

    /// Cooldown cache: volume_name -> last_migration_time
    pub cooldown_cache: DashMap<String, DateTime<Utc>>,

    /// Default Mayastor namespace
    pub mayastor_namespace: String,
}

impl ControllerContext {
    /// Create a new controller context
    pub fn new(
        client: Client,
        metrics: Arc<MetricsWatcher>,
        migrator: Arc<Migrator>,
        max_concurrent_migrations: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            client,
            metrics,
            migrator,
            migration_semaphore: Semaphore::new(max_concurrent_migrations),
            cooldown_cache: DashMap::new(),
            mayastor_namespace: MAYASTOR_NAMESPACE.to_string(),
        })
    }
}

// =============================================================================
// Reconciler
// =============================================================================

/// Reconcile a StoragePolicy resource
#[instrument(skip(policy, ctx), fields(policy = %policy.name_any()))]
pub async fn reconcile(
    policy: Arc<StoragePolicy>,
    ctx: Arc<ControllerContext>,
) -> Result<Action, Error> {
    let name = policy.name_any();
    info!("Reconciling StoragePolicy: {}", name);

    // Check if policy is enabled
    if !policy.is_enabled() {
        debug!("Policy {} is disabled, skipping", name);
        return update_status_disabled(&policy, &ctx).await;
    }

    // Parse durations from spec
    let sampling_window = policy.sampling_window()?;
    let cooldown_period = policy.cooldown_period()?;

    // Get all PVs using the configured StorageClass
    let pv_api: Api<PersistentVolume> = Api::all(ctx.client.clone());
    let pvs = pv_api.list(&ListParams::default()).await?;

    // Filter PVs by StorageClass
    let managed_pvs: Vec<&PersistentVolume> = pvs
        .items
        .iter()
        .filter(|pv| {
            pv.spec
                .as_ref()
                .and_then(|s| s.storage_class_name.as_ref())
                .map(|sc| sc == &policy.spec.storage_class_name)
                .unwrap_or(false)
        })
        .filter(|pv| {
            // Apply volume selector if specified
            if let Some(selector) = &policy.spec.volume_selector {
                let labels = pv.labels();
                selector.matches(&labels)
            } else {
                true
            }
        })
        .collect();

    info!("Found {} managed PVs for policy {}", managed_pvs.len(), name);

    // Collect volume IDs
    let volume_ids: Vec<String> = managed_pvs
        .iter()
        .filter_map(|pv| get_volume_id(pv))
        .collect();

    // Get heat scores for all volumes
    let heat_scores = ctx
        .metrics
        .get_bulk_heat_scores(&volume_ids, sampling_window)
        .await;

    // Build status counters
    let mut hot_count = 0u32;
    let mut cold_count = 0u32;
    let mut migrations_triggered = 0u32;

    // Get pool selectors
    let nvme_labels = policy
        .spec
        .nvme_pool_selector
        .as_ref()
        .map(|s| s.match_labels.clone())
        .unwrap_or_else(|| {
            [("storage-tier".to_string(), "hot".to_string())]
                .into_iter()
                .collect()
        });

    let sata_labels = policy
        .spec
        .sata_pool_selector
        .as_ref()
        .map(|s| s.match_labels.clone())
        .unwrap_or_else(|| {
            [("storage-tier".to_string(), "cold".to_string())]
                .into_iter()
                .collect()
        });

    // Process each volume
    for (pv, score) in managed_pvs.iter().zip(heat_scores.iter()) {
        let pv_name = pv.name_any();
        let volume_id = match get_volume_id(pv) {
            Some(id) => id,
            None => continue,
        };

        // Determine current tier from annotations
        let current_tier = get_current_tier(pv);

        // Check thresholds
        let needs_nvme = score.is_hot(policy.spec.high_watermark_iops)
            && current_tier != Some("nvme".to_string());
        let needs_sata = score.is_cold(policy.spec.low_watermark_iops)
            && current_tier != Some("sata".to_string());

        // Update counters
        if current_tier == Some("nvme".to_string()) {
            hot_count += 1;
        } else {
            cold_count += 1;
        }

        // Decide if migration is needed
        let (target_tier, target_labels) = if needs_nvme {
            info!(
                "Volume {} is HOT ({:.0} IOPS > {}), needs NVMe",
                volume_id, score.score, policy.spec.high_watermark_iops
            );
            ("nvme", &nvme_labels)
        } else if needs_sata {
            info!(
                "Volume {} is COLD ({:.0} IOPS < {}), needs SATA",
                volume_id, score.score, policy.spec.low_watermark_iops
            );
            ("sata", &sata_labels)
        } else {
            debug!(
                "Volume {} at {:.0} IOPS - no migration needed",
                volume_id, score.score
            );
            continue;
        };

        // Check cooldown
        if is_in_cooldown(&volume_id, cooldown_period, pv, &ctx) {
            debug!("Volume {} is in cooldown period, skipping", volume_id);
            continue;
        }

        // Try to acquire migration permit
        let permit = match ctx.migration_semaphore.try_acquire() {
            Ok(p) => p,
            Err(_) => {
                debug!(
                    "Max concurrent migrations reached, deferring {}",
                    volume_id
                );
                continue;
            }
        };

        // Find target pool
        let target_pool = match ctx
            .migrator
            .find_pool_for_tier(target_tier, target_labels)
            .await
        {
            Ok(pool) => pool,
            Err(e) => {
                warn!("No suitable {} pool found: {}", target_tier, e);
                drop(permit);
                continue;
            }
        };

        // Trigger migration (in background)
        migrations_triggered += 1;

        if policy.is_dry_run() {
            info!(
                "[DRY-RUN] Would migrate {} to {} (pool: {})",
                volume_id, target_tier, target_pool
            );
            drop(permit);
        } else {
            let ctx = ctx.clone();
            let policy_name = name.clone();
            let volume_id = volume_id.clone();
            let target_pool = target_pool.clone();
            let target_tier = target_tier.to_string();
            let pv_name = pv_name.clone();
            let trigger_iops = score.score;

            tokio::spawn(async move {
                let result = ctx
                    .migrator
                    .migrate_volume(&volume_id, &target_pool, &ctx.mayastor_namespace)
                    .await;

                // Update cooldown cache
                ctx.cooldown_cache.insert(volume_id.clone(), Utc::now());

                // Update PV annotations
                if let Err(e) = update_pv_annotations(
                    &ctx.client,
                    &pv_name,
                    &target_tier,
                    &policy_name,
                ).await {
                    warn!("Failed to update PV annotations: {}", e);
                }

                // Log result
                match result {
                    Ok(r) if r.is_success() => {
                        info!(
                            "Migration completed: {} -> {} in {:?}",
                            volume_id, target_pool, r.duration
                        );
                        // Update policy status with migration history
                        if let Err(e) = add_migration_to_history(
                            &ctx.client,
                            &policy_name,
                            &volume_id,
                            &target_tier,
                            trigger_iops,
                            &r,
                        ).await {
                            warn!("Failed to update migration history: {}", e);
                        }
                    }
                    Ok(r) => {
                        warn!(
                            "Migration did not complete successfully: {} - {:?}",
                            volume_id, r.error
                        );
                    }
                    Err(e) => {
                        error!("Migration failed for {}: {}", volume_id, e);
                    }
                }

                drop(permit);
            });
        }
    }

    // Update status
    let status = StoragePolicyStatus {
        phase: PolicyPhase::Active,
        watched_volumes: managed_pvs.len() as u32,
        hot_volumes: hot_count,
        cold_volumes: cold_count,
        active_migrations: ctx.migrator.active_count() as u32,
        total_migrations: 0, // Updated by migration completion
        failed_migrations: 0,
        last_reconcile_time: Some(Utc::now()),
        conditions: vec![PolicyCondition {
            r#type: "Ready".to_string(),
            status: ConditionStatus::True,
            last_transition_time: Some(Utc::now()),
            reason: Some("Reconciled".to_string()),
            message: Some(format!(
                "Watching {} volumes, triggered {} migrations",
                managed_pvs.len(),
                migrations_triggered
            )),
        }],
        migration_history: vec![], // Preserved from existing status
    };

    update_status(&policy, &ctx, status).await?;

    // Requeue for next reconciliation
    Ok(Action::requeue(DEFAULT_REQUEUE_INTERVAL))
}

/// Handle reconciliation errors
pub fn error_policy(
    policy: Arc<StoragePolicy>,
    error: &Error,
    _ctx: Arc<ControllerContext>,
) -> Action {
    error!(
        "Reconciliation error for {}: {}",
        policy.name_any(),
        error
    );

    match error.action() {
        crate::error::ErrorAction::RequeueWithBackoff => {
            Action::requeue(Duration::from_secs(60))
        }
        crate::error::ErrorAction::RequeueAfter(d) => Action::requeue(d),
        crate::error::ErrorAction::NoRequeue => Action::await_change(),
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Get volume ID from PV
fn get_volume_id(pv: &PersistentVolume) -> Option<String> {
    // Try CSI volume handle first
    pv.spec
        .as_ref()?
        .csi
        .as_ref()
        .map(|csi| csi.volume_handle.clone())
        .or_else(|| pv.metadata.name.clone())
}

/// Get current tier from PV annotations
fn get_current_tier(pv: &PersistentVolume) -> Option<String> {
    pv.metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get(ANNOTATION_CURRENT_TIER))
        .cloned()
}

/// Check if volume is in cooldown period
fn is_in_cooldown(
    volume_id: &str,
    cooldown: Duration,
    pv: &PersistentVolume,
    ctx: &ControllerContext,
) -> bool {
    // Check in-memory cache first
    if let Some(last_migration) = ctx.cooldown_cache.get(volume_id) {
        let elapsed = Utc::now().signed_duration_since(*last_migration);
        if elapsed < chrono::Duration::from_std(cooldown).unwrap_or_default() {
            return true;
        }
    }

    // Check PV annotation
    if let Some(timestamp_str) = pv
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get(ANNOTATION_LAST_MIGRATION))
    {
        if let Ok(timestamp) = DateTime::parse_from_rfc3339(timestamp_str) {
            let elapsed = Utc::now().signed_duration_since(timestamp.with_timezone(&Utc));
            if elapsed < chrono::Duration::from_std(cooldown).unwrap_or_default() {
                return true;
            }
        }
    }

    false
}

/// Update PV annotations after migration
async fn update_pv_annotations(
    client: &Client,
    pv_name: &str,
    tier: &str,
    policy_name: &str,
) -> Result<()> {
    let pv_api: Api<PersistentVolume> = Api::all(client.clone());

    let patch = serde_json::json!({
        "metadata": {
            "annotations": {
                ANNOTATION_LAST_MIGRATION: Utc::now().to_rfc3339(),
                ANNOTATION_CURRENT_TIER: tier,
                ANNOTATION_MANAGED_BY: policy_name,
            }
        }
    });

    pv_api
        .patch(
            pv_name,
            &PatchParams::apply("smart-storage-operator"),
            &Patch::Merge(&patch),
        )
        .await?;

    Ok(())
}

/// Update StoragePolicy status
async fn update_status(
    policy: &StoragePolicy,
    ctx: &ControllerContext,
    status: StoragePolicyStatus,
) -> Result<()> {
    let api: Api<StoragePolicy> = Api::all(ctx.client.clone());

    let patch = serde_json::json!({
        "status": status
    });

    api.patch_status(
        &policy.name_any(),
        &PatchParams::apply("smart-storage-operator"),
        &Patch::Merge(&patch),
    )
    .await?;

    Ok(())
}

/// Update status to disabled
async fn update_status_disabled(
    policy: &StoragePolicy,
    ctx: &ControllerContext,
) -> Result<Action, Error> {
    let status = StoragePolicyStatus {
        phase: PolicyPhase::Disabled,
        last_reconcile_time: Some(Utc::now()),
        conditions: vec![PolicyCondition {
            r#type: "Ready".to_string(),
            status: ConditionStatus::False,
            last_transition_time: Some(Utc::now()),
            reason: Some("Disabled".to_string()),
            message: Some("Policy is disabled".to_string()),
        }],
        ..Default::default()
    };

    update_status(policy, ctx, status).await?;
    Ok(Action::await_change())
}

/// Add migration to policy history
async fn add_migration_to_history(
    client: &Client,
    policy_name: &str,
    volume_name: &str,
    to_tier: &str,
    trigger_iops: f64,
    result: &MigrationResult,
) -> Result<()> {
    let api: Api<StoragePolicy> = Api::all(client.clone());

    // Get current policy to preserve history
    let policy = api.get(policy_name).await?;
    let mut history = policy
        .status
        .map(|s| s.migration_history)
        .unwrap_or_default();

    // Add new entry
    let entry = MigrationHistoryEntry {
        volume_name: volume_name.to_string(),
        timestamp: Utc::now(),
        from_tier: result.source_pool.clone(),
        to_tier: to_tier.to_string(),
        trigger_iops,
        duration: format!("{:?}", result.duration),
        success: result.is_success(),
        error: result.error.clone(),
    };

    history.insert(0, entry);
    history.truncate(50); // Keep last 50

    let patch = serde_json::json!({
        "status": {
            "migrationHistory": history,
            "totalMigrations": policy.status.as_ref().map(|s| s.total_migrations + 1).unwrap_or(1)
        }
    });

    api.patch_status(
        policy_name,
        &PatchParams::apply("smart-storage-operator"),
        &Patch::Merge(&patch),
    )
    .await?;

    Ok(())
}

// =============================================================================
// Controller Setup
// =============================================================================

/// Run the StoragePolicy controller
pub async fn run(ctx: Arc<ControllerContext>) -> Result<()> {
    let api: Api<StoragePolicy> = Api::all(ctx.client.clone());

    info!("Starting StoragePolicy controller");

    Controller::new(api, Config::default())
        .shutdown_on_signal()
        .run(reconcile, error_policy, ctx)
        .for_each(|result| async move {
            match result {
                Ok((obj, _action)) => {
                    debug!("Reconciled: {}", obj.name);
                }
                Err(e) => {
                    error!("Reconciliation error: {:?}", e);
                }
            }
        })
        .await;

    info!("StoragePolicy controller stopped");
    Ok(())
}
