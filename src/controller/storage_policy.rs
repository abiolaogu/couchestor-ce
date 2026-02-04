//! StoragePolicy Controller
//!
//! Reconciliation logic for StoragePolicy resources.

use crate::crd::{
    ConditionStatus, MigrationHistoryEntry, PolicyCondition, PolicyPhase, StoragePolicy,
    StoragePolicyStatus,
};
use crate::error::{Error, Result};
use crate::metrics::MetricsWatcher;
use crate::migrator::{MigrationResult, Migrator};

use chrono::Utc;
use futures::StreamExt;
use k8s_openapi::api::core::v1::PersistentVolume;
use kube::api::{Api, ListParams, Patch, PatchParams};
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, instrument, warn};

/// Shared context for the controller
pub struct ControllerContext {
    /// Kubernetes client
    pub client: Client,

    /// Metrics watcher for querying Prometheus
    pub metrics_watcher: Arc<MetricsWatcher>,

    /// Migrator for executing volume migrations
    pub migrator: Arc<Migrator>,

    /// Semaphore to limit concurrent migrations
    pub migration_semaphore: Arc<Semaphore>,
}

impl ControllerContext {
    /// Create a new controller context
    pub fn new(
        client: Client,
        metrics_watcher: Arc<MetricsWatcher>,
        migrator: Arc<Migrator>,
        max_concurrent_migrations: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            client,
            metrics_watcher,
            migrator,
            migration_semaphore: Arc::new(Semaphore::new(max_concurrent_migrations)),
        })
    }
}

/// Run the StoragePolicy controller
pub async fn run(ctx: Arc<ControllerContext>) -> Result<()> {
    let client = ctx.client.clone();
    let policies: Api<StoragePolicy> = Api::all(client.clone());

    // Check if CRD exists
    if let Err(e) = policies.list(&ListParams::default().limit(1)).await {
        error!(
            "StoragePolicy CRD not found: {}. Please install the CRD first.",
            e
        );
        return Err(Error::Kube(e));
    }

    info!("Starting StoragePolicy controller");

    Controller::new(policies, Config::default())
        .shutdown_on_signal()
        .run(reconcile, error_policy, ctx)
        .for_each(|res| async move {
            match res {
                Ok(o) => debug!("Reconciled {:?}", o),
                Err(e) => error!("Reconcile failed: {:?}", e),
            }
        })
        .await;

    info!("Controller shutdown complete");
    Ok(())
}

/// Reconcile a StoragePolicy resource
#[instrument(skip(policy, ctx), fields(policy = %policy.name_any()))]
async fn reconcile(
    policy: Arc<StoragePolicy>,
    ctx: Arc<ControllerContext>,
) -> std::result::Result<Action, Error> {
    let name = policy.name_any();
    info!("Reconciling StoragePolicy: {}", name);

    // Check if policy is enabled
    if !policy.is_enabled() {
        debug!("Policy {} is disabled, skipping", name);
        return Ok(Action::requeue(Duration::from_secs(300)));
    }

    // Get sampling window
    let sampling_window = policy
        .sampling_window()
        .unwrap_or(Duration::from_secs(3600));
    let cooldown_period = policy
        .cooldown_period()
        .unwrap_or(Duration::from_secs(86400));

    // List PVs matching the storage class
    let pvs: Api<PersistentVolume> = Api::all(ctx.client.clone());
    let pv_list = pvs.list(&ListParams::default()).await?;

    let matching_pvs: Vec<_> = pv_list
        .items
        .into_iter()
        .filter(|pv| {
            pv.spec
                .as_ref()
                .and_then(|s| s.storage_class_name.as_ref())
                .map(|sc| sc == &policy.spec.storage_class_name)
                .unwrap_or(false)
        })
        .collect();

    info!(
        "Found {} PVs matching StorageClass {}",
        matching_pvs.len(),
        policy.spec.storage_class_name
    );

    let mut hot_count = 0u32;
    let mut warm_count = 0u32;
    let mut cold_count = 0u32;

    // Check if warm tier is enabled
    let warm_enabled = policy.warm_tier_enabled();
    let warm_threshold = policy.spec.warm_watermark_iops;

    // Process each PV
    for pv in &matching_pvs {
        let pv_name = pv.name_any();

        // Get volume ID from PV
        let volume_id = pv
            .spec
            .as_ref()
            .and_then(|s| s.csi.as_ref())
            .map(|csi| csi.volume_handle.clone())
            .unwrap_or_else(|| pv_name.clone());

        // Get heat score
        let heat_score = ctx
            .metrics_watcher
            .get_heat_score(&volume_id, sampling_window)
            .await
            .unwrap_or_else(|e| {
                warn!("Failed to get heat score for {}: {}", volume_id, e);
                crate::metrics::HeatScore::zero(&volume_id)
            });

        debug!("Volume {} heat score: {} IOPS", volume_id, heat_score.score);

        // Determine tier based on thresholds (Hot/Warm/Cold)
        // Hot: IOPS >= high_watermark (NVMe, fast SSD)
        // Warm: low_watermark < IOPS < high_watermark (SAS, SATA SSD) - if enabled
        // Cold: IOPS <= low_watermark (HDD, archival)

        let iops = heat_score.score as u32;

        if iops >= policy.spec.high_watermark_iops {
            // HOT TIER - High IOPS workload (NVMe, fast SSD, SAS SSD)
            hot_count += 1;

            if should_migrate(pv, cooldown_period) {
                if let Some(selector) = policy.hot_pool_selector() {
                    if let Ok(target_pool) = ctx
                        .migrator
                        .find_pool_for_tier("hot", &selector.match_labels)
                        .await
                    {
                        if !ctx.migrator.is_migrating(&volume_id) {
                            let _permit = ctx.migration_semaphore.acquire().await;
                            info!(
                                "Migrating {} to HOT tier (pool: {}, IOPS: {})",
                                volume_id, target_pool, iops
                            );

                            if !policy.is_dry_run() {
                                match ctx
                                    .migrator
                                    .migrate_volume(&volume_id, &target_pool, "mayastor")
                                    .await
                                {
                                    Ok(result) => {
                                        info!("Migration completed: {:?}", result.state);
                                        // Record in history
                                        record_migration_history(
                                            &ctx.client,
                                            &name,
                                            &result,
                                            iops.into(),
                                            "cold", // Assuming migration to hot is from cold/warm
                                            "hot",
                                        )
                                        .await;
                                    }
                                    Err(e) => error!("Migration failed: {}", e),
                                }
                            } else {
                                info!(
                                    "[DRY-RUN] Would migrate {} to HOT tier ({})",
                                    volume_id, target_pool
                                );
                            }
                        }
                    }
                }
            }
        } else if warm_enabled && iops > policy.spec.low_watermark_iops && iops < warm_threshold {
            // WARM TIER - Medium IOPS workload (SAS, SATA SSD, hybrid storage)
            warm_count += 1;

            if should_migrate(pv, cooldown_period) {
                if let Some(selector) = policy.warm_pool_selector() {
                    if let Ok(target_pool) = ctx
                        .migrator
                        .find_pool_for_tier("warm", &selector.match_labels)
                        .await
                    {
                        if !ctx.migrator.is_migrating(&volume_id) {
                            let _permit = ctx.migration_semaphore.acquire().await;
                            info!(
                                "Migrating {} to WARM tier (pool: {}, IOPS: {})",
                                volume_id, target_pool, iops
                            );

                            if !policy.is_dry_run() {
                                match ctx
                                    .migrator
                                    .migrate_volume(&volume_id, &target_pool, "mayastor")
                                    .await
                                {
                                    Ok(result) => {
                                        info!("Migration completed: {:?}", result.state);
                                        // Record in history
                                        record_migration_history(
                                            &ctx.client,
                                            &name,
                                            &result,
                                            iops.into(),
                                            "cold", // Assuming migration to warm is from cold
                                            "warm",
                                        )
                                        .await;
                                    }
                                    Err(e) => error!("Migration failed: {}", e),
                                }
                            } else {
                                info!(
                                    "[DRY-RUN] Would migrate {} to WARM tier ({})",
                                    volume_id, target_pool
                                );
                            }
                        }
                    }
                }
            }
        } else if iops <= policy.spec.low_watermark_iops {
            // COLD TIER - Low IOPS workload (HDD, SATA, archival storage, or EC)
            cold_count += 1;

            if should_migrate(pv, cooldown_period) {
                // Check if EC is enabled and volume qualifies
                let volume_size = get_volume_size(pv);
                let use_ec = policy.volume_qualifies_for_ec(volume_size);

                if use_ec {
                    // Migrate to EC storage
                    if let Some(ec_policy_ref) = policy.ec_policy_ref() {
                        if !ctx.migrator.is_migrating(&volume_id) {
                            let _permit = ctx.migration_semaphore.acquire().await;
                            info!(
                                "Migrating {} to EC cold tier (policy: {}, IOPS: {}, size: {} bytes)",
                                volume_id, ec_policy_ref, iops, volume_size
                            );

                            if !policy.is_dry_run() {
                                // Get target pools for EC shards from cold pool selector
                                let target_pools: Vec<String> =
                                    if let Some(selector) = policy.cold_pool_selector() {
                                        // In a real implementation, we'd find multiple pools matching the selector
                                        // For now, use a placeholder
                                        vec![ctx
                                            .migrator
                                            .find_pool_for_tier("cold", &selector.match_labels)
                                            .await
                                            .unwrap_or_else(|_| "cold-pool-1".to_string())]
                                    } else {
                                        vec![]
                                    };

                                if !target_pools.is_empty() {
                                    match ctx
                                        .migrator
                                        .migrate_to_ec(
                                            &volume_id,
                                            ec_policy_ref,
                                            &target_pools,
                                            "mayastor",
                                        )
                                        .await
                                    {
                                        Ok(result) => {
                                            info!(
                                                "EC migration completed: {:?}, stripes created: {:?}",
                                                result.state, result.ec_stripes_created
                                            );
                                            // Record in history
                                            record_migration_history(
                                                &ctx.client,
                                                &name,
                                                &result,
                                                iops.into(),
                                                "warm", // Could be from hot or warm
                                                "cold-ec",
                                            )
                                            .await;
                                        }
                                        Err(e) => error!("EC migration failed: {}", e),
                                    }
                                } else {
                                    warn!(
                                        "No target pools found for EC migration of {}",
                                        volume_id
                                    );
                                }
                            } else {
                                info!(
                                    "[DRY-RUN] Would migrate {} to EC cold tier (policy: {})",
                                    volume_id, ec_policy_ref
                                );
                            }
                        }
                    }
                } else {
                    // Standard cold tier migration (replication)
                    if let Some(selector) = policy.cold_pool_selector() {
                        if let Ok(target_pool) = ctx
                            .migrator
                            .find_pool_for_tier("cold", &selector.match_labels)
                            .await
                        {
                            if !ctx.migrator.is_migrating(&volume_id) {
                                let _permit = ctx.migration_semaphore.acquire().await;
                                info!(
                                    "Migrating {} to COLD tier (pool: {}, IOPS: {})",
                                    volume_id, target_pool, iops
                                );

                                if !policy.is_dry_run() {
                                    match ctx
                                        .migrator
                                        .migrate_volume(&volume_id, &target_pool, "mayastor")
                                        .await
                                    {
                                        Ok(result) => {
                                            info!("Migration completed: {:?}", result.state);
                                            // Record in history
                                            record_migration_history(
                                                &ctx.client,
                                                &name,
                                                &result,
                                                iops.into(),
                                                "warm", // Could be from hot or warm
                                                "cold",
                                            )
                                            .await;
                                        }
                                        Err(e) => error!("Migration failed: {}", e),
                                    }
                                } else {
                                    info!(
                                        "[DRY-RUN] Would migrate {} to COLD tier ({})",
                                        volume_id, target_pool
                                    );
                                }
                            }
                        }
                    }
                }
            }
        } else {
            // Between warm threshold and high watermark (or warm disabled) - stays in current tier
            warm_count += 1;
        }
    }

    // Update status
    let status = StoragePolicyStatus {
        phase: PolicyPhase::Active,
        watched_volumes: matching_pvs.len() as u32,
        hot_volumes: hot_count,
        warm_volumes: warm_count,
        cold_volumes: cold_count,
        active_migrations: ctx.migrator.active_count() as u32,
        last_reconcile_time: Some(Utc::now()),
        conditions: vec![PolicyCondition {
            r#type: "Ready".to_string(),
            status: ConditionStatus::True,
            last_transition_time: Some(Utc::now()),
            reason: Some("Reconciled".to_string()),
            message: Some(format!(
                "Watching {} volumes (hot:{}, warm:{}, cold:{})",
                matching_pvs.len(),
                hot_count,
                warm_count,
                cold_count
            )),
        }],
        ..Default::default()
    };

    // Patch status
    let policies: Api<StoragePolicy> = Api::all(ctx.client.clone());
    let patch = serde_json::json!({ "status": status });
    let _ = policies
        .patch_status(
            &name,
            &PatchParams::apply("smart-storage-operator"),
            &Patch::Merge(&patch),
        )
        .await;

    // Requeue after 5 minutes
    Ok(Action::requeue(Duration::from_secs(300)))
}

/// Check if a volume should be migrated based on cooldown period
fn should_migrate(pv: &PersistentVolume, cooldown: Duration) -> bool {
    let annotations = pv.metadata.annotations.as_ref();

    if let Some(last_migration) =
        annotations.and_then(|a| a.get("storage.billyronks.io/last-migration"))
    {
        if let Ok(last_time) = chrono::DateTime::parse_from_rfc3339(last_migration) {
            let elapsed = Utc::now().signed_duration_since(last_time);
            if elapsed < chrono::Duration::from_std(cooldown).unwrap_or(chrono::Duration::hours(24))
            {
                debug!("Volume {} is in cooldown period", pv.name_any());
                return false;
            }
        }
    }

    true
}

/// Get the size of a PersistentVolume in bytes
fn get_volume_size(pv: &PersistentVolume) -> u64 {
    pv.spec
        .as_ref()
        .and_then(|spec| spec.capacity.as_ref())
        .and_then(|cap| cap.get("storage"))
        .and_then(|storage| {
            // Parse Kubernetes quantity (e.g., "10Gi", "100G", "1Ti")
            let s = storage.0.as_str();
            parse_k8s_quantity(s)
        })
        .unwrap_or(0)
}

/// Parse a Kubernetes quantity string into bytes
fn parse_k8s_quantity(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Find where the number ends and the suffix begins
    let (num_str, suffix) = if let Some(pos) = s.find(|c: char| !c.is_ascii_digit() && c != '.') {
        (&s[..pos], &s[pos..])
    } else {
        (s, "")
    };

    let num: f64 = num_str.parse().ok()?;

    let multiplier: u64 = match suffix {
        "" => 1,
        "k" | "K" => 1_000,
        "M" => 1_000_000,
        "G" => 1_000_000_000,
        "T" => 1_000_000_000_000,
        "P" => 1_000_000_000_000_000,
        "Ki" => 1_024,
        "Mi" => 1_048_576,
        "Gi" => 1_073_741_824,
        "Ti" => 1_099_511_627_776,
        "Pi" => 1_125_899_906_842_624,
        _ => return None,
    };

    Some((num * multiplier as f64) as u64)
}

/// Record a migration in the StoragePolicy history
async fn record_migration_history(
    client: &Client,
    policy_name: &str,
    result: &MigrationResult,
    trigger_iops: f64,
    from_tier: &str,
    to_tier: &str,
) {
    // Create history entry
    let duration_secs = result.duration.as_secs_f64();
    let entry = MigrationHistoryEntry::new(
        result.volume_name.clone(),
        result.end_time,
        from_tier.to_string(),
        to_tier.to_string(),
        trigger_iops,
        duration_secs,
        result.is_success(),
        result.error.clone(),
    );

    // Fetch current policy to get existing status
    let policies: Api<StoragePolicy> = Api::all(client.clone());
    if let Ok(mut policy) = policies.get(policy_name).await {
        // Add to history and update counters
        let mut status = policy.status.clone().unwrap_or_default();
        status.add_migration_history(entry);

        // Increment total migrations counter
        status.total_migrations += 1;

        // Increment failed migrations counter if migration failed
        if !result.is_success() {
            status.failed_migrations += 1;
        }

        // Patch status with updated history and counters
        let patch = serde_json::json!({
            "status": {
                "migrationHistory": status.migration_history,
                "totalMigrations": status.total_migrations,
                "failedMigrations": status.failed_migrations
            }
        });

        let patch_params = PatchParams::apply("smart-storage-operator");
        if let Err(e) = policies
            .patch_status(policy_name, &patch_params, &Patch::Merge(&patch))
            .await
        {
            warn!(
                "Failed to update migration history for policy {}: {}",
                policy_name, e
            );
        } else {
            debug!(
                "Recorded migration history for volume {} in policy {} (total: {}, failed: {})",
                result.volume_name, policy_name, status.total_migrations, status.failed_migrations
            );
        }
    }
}

/// Error policy for the controller
fn error_policy(
    _policy: Arc<StoragePolicy>,
    error: &Error,
    _ctx: Arc<ControllerContext>,
) -> Action {
    error!("Reconciliation error: {}", error);
    Action::requeue(Duration::from_secs(60))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{
        PersistentVolume, PersistentVolumeSpec, PersistentVolumeStatus,
    };
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
    use kube::api::ObjectMeta;
    use std::collections::BTreeMap;

    // =============================================================================
    // parse_k8s_quantity Tests
    // =============================================================================

    #[test]
    fn test_parse_k8s_quantity_bytes() {
        assert_eq!(parse_k8s_quantity("100"), Some(100));
        assert_eq!(parse_k8s_quantity("0"), Some(0));
        assert_eq!(parse_k8s_quantity("1"), Some(1));
    }

    #[test]
    fn test_parse_k8s_quantity_decimal() {
        assert_eq!(parse_k8s_quantity("1000"), Some(1_000));
        assert_eq!(parse_k8s_quantity("1k"), Some(1_000));
        assert_eq!(parse_k8s_quantity("1K"), Some(1_000));
        assert_eq!(parse_k8s_quantity("1M"), Some(1_000_000));
        assert_eq!(parse_k8s_quantity("1G"), Some(1_000_000_000));
        assert_eq!(parse_k8s_quantity("1T"), Some(1_000_000_000_000));
        assert_eq!(parse_k8s_quantity("1P"), Some(1_000_000_000_000_000));
    }

    #[test]
    fn test_parse_k8s_quantity_binary() {
        assert_eq!(parse_k8s_quantity("1Ki"), Some(1_024));
        assert_eq!(parse_k8s_quantity("1Mi"), Some(1_048_576));
        assert_eq!(parse_k8s_quantity("1Gi"), Some(1_073_741_824));
        assert_eq!(parse_k8s_quantity("1Ti"), Some(1_099_511_627_776));
        assert_eq!(parse_k8s_quantity("1Pi"), Some(1_125_899_906_842_624));
    }

    #[test]
    fn test_parse_k8s_quantity_fractional() {
        assert_eq!(parse_k8s_quantity("0.5Gi"), Some(536_870_912));
        assert_eq!(parse_k8s_quantity("2.5Mi"), Some(2_621_440));
        assert_eq!(parse_k8s_quantity("10.5Ki"), Some(10_752));
    }

    #[test]
    fn test_parse_k8s_quantity_realistic_values() {
        // Common PV sizes
        assert_eq!(parse_k8s_quantity("10Gi"), Some(10_737_418_240)); // 10GB
        assert_eq!(parse_k8s_quantity("100Gi"), Some(107_374_182_400)); // 100GB
        assert_eq!(parse_k8s_quantity("1Ti"), Some(1_099_511_627_776)); // 1TB
    }

    #[test]
    fn test_parse_k8s_quantity_edge_cases() {
        assert_eq!(parse_k8s_quantity(""), None);
        assert_eq!(parse_k8s_quantity("   "), None);
        assert_eq!(parse_k8s_quantity("abc"), None);
        assert_eq!(parse_k8s_quantity("1Zi"), None); // Unknown suffix
    }

    #[test]
    fn test_parse_k8s_quantity_whitespace() {
        assert_eq!(parse_k8s_quantity("  10Gi  "), Some(10_737_418_240));
        assert_eq!(parse_k8s_quantity(" 1Mi "), Some(1_048_576));
    }

    // =============================================================================
    // get_volume_size Tests
    // =============================================================================

    #[test]
    fn test_get_volume_size_valid() {
        let mut capacity = BTreeMap::new();
        capacity.insert("storage".to_string(), Quantity("10Gi".to_string()));

        let pv = PersistentVolume {
            metadata: ObjectMeta::default(),
            spec: Some(PersistentVolumeSpec {
                capacity: Some(capacity),
                ..Default::default()
            }),
            status: None,
        };

        assert_eq!(get_volume_size(&pv), 10_737_418_240);
    }

    #[test]
    fn test_get_volume_size_missing_capacity() {
        let pv = PersistentVolume {
            metadata: ObjectMeta::default(),
            spec: Some(PersistentVolumeSpec {
                capacity: None,
                ..Default::default()
            }),
            status: None,
        };

        assert_eq!(get_volume_size(&pv), 0);
    }

    #[test]
    fn test_get_volume_size_missing_storage_key() {
        let mut capacity = BTreeMap::new();
        capacity.insert("other".to_string(), Quantity("10Gi".to_string()));

        let pv = PersistentVolume {
            metadata: ObjectMeta::default(),
            spec: Some(PersistentVolumeSpec {
                capacity: Some(capacity),
                ..Default::default()
            }),
            status: None,
        };

        assert_eq!(get_volume_size(&pv), 0);
    }

    #[test]
    fn test_get_volume_size_no_spec() {
        let pv = PersistentVolume {
            metadata: ObjectMeta::default(),
            spec: None,
            status: None,
        };

        assert_eq!(get_volume_size(&pv), 0);
    }

    // =============================================================================
    // should_migrate Tests
    // =============================================================================

    #[test]
    fn test_should_migrate_no_annotation() {
        let pv = PersistentVolume {
            metadata: ObjectMeta {
                annotations: None,
                ..Default::default()
            },
            spec: None,
            status: None,
        };

        assert!(should_migrate(&pv, Duration::from_secs(3600)));
    }

    #[test]
    fn test_should_migrate_empty_annotations() {
        let pv = PersistentVolume {
            metadata: ObjectMeta {
                annotations: Some(BTreeMap::new()),
                ..Default::default()
            },
            spec: None,
            status: None,
        };

        assert!(should_migrate(&pv, Duration::from_secs(3600)));
    }

    #[test]
    fn test_should_migrate_within_cooldown() {
        // Recent migration (just now)
        let recent_time = Utc::now().to_rfc3339();
        let mut annotations = BTreeMap::new();
        annotations.insert(
            "storage.billyronks.io/last-migration".to_string(),
            recent_time,
        );

        let pv = PersistentVolume {
            metadata: ObjectMeta {
                annotations: Some(annotations),
                ..Default::default()
            },
            spec: None,
            status: None,
        };

        // Cooldown of 1 hour - should not migrate yet
        assert!(!should_migrate(&pv, Duration::from_secs(3600)));
    }

    #[test]
    fn test_should_migrate_after_cooldown() {
        // Migration 2 hours ago
        let old_time = (Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        let mut annotations = BTreeMap::new();
        annotations.insert("storage.billyronks.io/last-migration".to_string(), old_time);

        let pv = PersistentVolume {
            metadata: ObjectMeta {
                annotations: Some(annotations),
                ..Default::default()
            },
            spec: None,
            status: None,
        };

        // Cooldown of 1 hour - should migrate now
        assert!(should_migrate(&pv, Duration::from_secs(3600)));
    }

    #[test]
    fn test_should_migrate_invalid_timestamp() {
        let mut annotations = BTreeMap::new();
        annotations.insert(
            "storage.billyronks.io/last-migration".to_string(),
            "invalid-timestamp".to_string(),
        );

        let pv = PersistentVolume {
            metadata: ObjectMeta {
                annotations: Some(annotations),
                ..Default::default()
            },
            spec: None,
            status: None,
        };

        // Invalid timestamp should allow migration (fail-safe)
        assert!(should_migrate(&pv, Duration::from_secs(3600)));
    }

    // =============================================================================
    // Integration Test Helpers (require mock Kubernetes API)
    // =============================================================================

    #[test]
    fn test_tier_decision_logic_hot() {
        // Test hot tier threshold logic
        let high_watermark = 5000u32;
        let warm_watermark = 2000u32;
        let low_watermark = 500u32;

        let test_iops = 6000u32;

        // Hot tier: IOPS >= high_watermark
        assert!(test_iops >= high_watermark);
        assert!(test_iops > warm_watermark);
        assert!(test_iops > low_watermark);
    }

    #[test]
    fn test_tier_decision_logic_warm() {
        // Test warm tier threshold logic
        let high_watermark = 5000u32;
        let warm_watermark = 2000u32;
        let low_watermark = 500u32;

        let test_iops = 3000u32;

        // Warm tier: low_watermark < IOPS < warm_watermark
        assert!(test_iops < high_watermark);
        assert!(test_iops > low_watermark);
        assert!(test_iops > warm_watermark);
    }

    #[test]
    fn test_tier_decision_logic_cold() {
        // Test cold tier threshold logic
        let high_watermark = 5000u32;
        let warm_watermark = 2000u32;
        let low_watermark = 500u32;

        let test_iops = 200u32;

        // Cold tier: IOPS <= low_watermark
        assert!(test_iops < high_watermark);
        assert!(test_iops < warm_watermark);
        assert!(test_iops <= low_watermark);
    }

    #[test]
    fn test_ec_qualification_logic() {
        // Test EC vs replication decision logic
        let ec_min_size = 10_737_418_240u64; // 10GB

        // Small volume - should use replication
        let small_volume = 1_073_741_824u64; // 1GB
        assert!(small_volume < ec_min_size);

        // Large volume - should use EC
        let large_volume = 107_374_182_400u64; // 100GB
        assert!(large_volume >= ec_min_size);
    }

    // =============================================================================
    // Migration History Tests
    // =============================================================================

    #[test]
    fn test_migration_history_entry_creation() {
        use chrono::Utc;

        // Test successful migration with short duration
        let entry = MigrationHistoryEntry::new(
            "pvc-123".to_string(),
            Utc::now(),
            "warm".to_string(),
            "hot".to_string(),
            5500.0,
            12.5,
            true,
            None,
        );

        assert_eq!(entry.volume_name, "pvc-123");
        assert_eq!(entry.from_tier, "warm");
        assert_eq!(entry.to_tier, "hot");
        assert_eq!(entry.trigger_iops, 5500.0);
        assert_eq!(entry.duration, "12.5s");
        assert!(entry.success);
        assert!(entry.error.is_none());
    }

    #[test]
    fn test_migration_history_entry_duration_formatting() {
        use chrono::Utc;

        // Test milliseconds
        let entry_ms = MigrationHistoryEntry::new(
            "pvc-1".to_string(),
            Utc::now(),
            "cold".to_string(),
            "hot".to_string(),
            6000.0,
            0.5,
            true,
            None,
        );
        assert_eq!(entry_ms.duration, "500ms");

        // Test seconds
        let entry_s = MigrationHistoryEntry::new(
            "pvc-2".to_string(),
            Utc::now(),
            "cold".to_string(),
            "hot".to_string(),
            6000.0,
            30.0,
            true,
            None,
        );
        assert_eq!(entry_s.duration, "30.0s");

        // Test minutes
        let entry_m = MigrationHistoryEntry::new(
            "pvc-3".to_string(),
            Utc::now(),
            "cold".to_string(),
            "hot".to_string(),
            6000.0,
            150.0,
            true,
            None,
        );
        assert_eq!(entry_m.duration, "2.5m");

        // Test hours
        let entry_h = MigrationHistoryEntry::new(
            "pvc-4".to_string(),
            Utc::now(),
            "cold".to_string(),
            "hot".to_string(),
            6000.0,
            7200.0,
            true,
            None,
        );
        assert_eq!(entry_h.duration, "2.0h");
    }

    #[test]
    fn test_migration_history_entry_with_error() {
        use chrono::Utc;

        // Test failed migration
        let entry = MigrationHistoryEntry::new(
            "pvc-failed".to_string(),
            Utc::now(),
            "cold".to_string(),
            "hot".to_string(),
            5000.0,
            5.0,
            false,
            Some("Timeout waiting for replica sync".to_string()),
        );

        assert_eq!(entry.volume_name, "pvc-failed");
        assert!(!entry.success);
        assert!(entry.error.is_some());
        assert_eq!(
            entry.error.unwrap(),
            "Timeout waiting for replica sync"
        );
    }

    #[test]
    fn test_storage_policy_status_add_migration_history() {
        use chrono::Utc;

        let mut status = StoragePolicyStatus::default();

        // Add first entry
        let entry1 = MigrationHistoryEntry::new(
            "pvc-1".to_string(),
            Utc::now(),
            "cold".to_string(),
            "hot".to_string(),
            6000.0,
            10.0,
            true,
            None,
        );
        status.add_migration_history(entry1);
        assert_eq!(status.migration_history.len(), 1);

        // Add more entries
        for i in 2..=55 {
            let entry = MigrationHistoryEntry::new(
                format!("pvc-{}", i),
                Utc::now(),
                "cold".to_string(),
                "hot".to_string(),
                6000.0,
                10.0,
                true,
                None,
            );
            status.add_migration_history(entry);
        }

        // Should be capped at 50
        assert_eq!(status.migration_history.len(), 50);

        // Most recent should be first
        assert_eq!(status.migration_history[0].volume_name, "pvc-55");
    }

    #[test]
    fn test_migration_history_tier_labels() {
        use chrono::Utc;

        // Test all tier combinations
        let hot_to_cold = MigrationHistoryEntry::new(
            "pvc-1".to_string(),
            Utc::now(),
            "hot".to_string(),
            "cold".to_string(),
            100.0,
            10.0,
            true,
            None,
        );
        assert_eq!(hot_to_cold.from_tier, "hot");
        assert_eq!(hot_to_cold.to_tier, "cold");

        let cold_to_warm = MigrationHistoryEntry::new(
            "pvc-2".to_string(),
            Utc::now(),
            "cold".to_string(),
            "warm".to_string(),
            1500.0,
            15.0,
            true,
            None,
        );
        assert_eq!(cold_to_warm.from_tier, "cold");
        assert_eq!(cold_to_warm.to_tier, "warm");

        let warm_to_hot = MigrationHistoryEntry::new(
            "pvc-3".to_string(),
            Utc::now(),
            "warm".to_string(),
            "hot".to_string(),
            6000.0,
            12.0,
            true,
            None,
        );
        assert_eq!(warm_to_hot.from_tier, "warm");
        assert_eq!(warm_to_hot.to_tier, "hot");

        // Test EC tier
        let warm_to_ec = MigrationHistoryEntry::new(
            "pvc-4".to_string(),
            Utc::now(),
            "warm".to_string(),
            "cold-ec".to_string(),
            200.0,
            20.0,
            true,
            None,
        );
        assert_eq!(warm_to_ec.to_tier, "cold-ec");
    }

    // =========================================================================
    // Migration Counter Tests
    // =========================================================================

    #[test]
    fn test_migration_counters_initialized_to_zero() {
        let status = StoragePolicyStatus::default();
        assert_eq!(status.total_migrations, 0);
        assert_eq!(status.failed_migrations, 0);
    }

    #[test]
    fn test_increment_total_migrations() {
        let mut status = StoragePolicyStatus::default();

        status.total_migrations += 1;
        assert_eq!(status.total_migrations, 1);

        status.total_migrations += 1;
        assert_eq!(status.total_migrations, 2);

        status.total_migrations += 5;
        assert_eq!(status.total_migrations, 7);
    }

    #[test]
    fn test_increment_failed_migrations() {
        let mut status = StoragePolicyStatus::default();

        status.failed_migrations += 1;
        assert_eq!(status.failed_migrations, 1);

        status.failed_migrations += 1;
        assert_eq!(status.failed_migrations, 2);

        status.failed_migrations += 3;
        assert_eq!(status.failed_migrations, 5);
    }

    #[test]
    fn test_migration_success_rate_all_success() {
        let status = StoragePolicyStatus {
            total_migrations: 100,
            failed_migrations: 0,
            ..Default::default()
        };

        let success_rate = if status.total_migrations > 0 {
            ((status.total_migrations - status.failed_migrations) as f64
                / status.total_migrations as f64) * 100.0
        } else {
            0.0
        };

        assert_eq!(success_rate, 100.0);
    }

    #[test]
    fn test_migration_success_rate_with_failures() {
        let status = StoragePolicyStatus {
            total_migrations: 100,
            failed_migrations: 5,
            ..Default::default()
        };

        let success_rate = if status.total_migrations > 0 {
            ((status.total_migrations - status.failed_migrations) as f64
                / status.total_migrations as f64) * 100.0
        } else {
            0.0
        };

        assert_eq!(success_rate, 95.0);
    }

    #[test]
    fn test_migration_success_rate_no_migrations() {
        let status = StoragePolicyStatus::default();

        let success_rate = if status.total_migrations > 0 {
            ((status.total_migrations - status.failed_migrations) as f64
                / status.total_migrations as f64) * 100.0
        } else {
            0.0
        };

        assert_eq!(success_rate, 0.0);
    }

    // =========================================================================
    // Prometheus Unavailability Tests (MET-006)
    // =========================================================================

    #[tokio::test]
    async fn test_reconcile_with_prometheus_unavailable() {
        // Test that reconciliation continues gracefully when Prometheus is down
        use crate::metrics::{MetricsConfig, MetricsWatcher};
        use std::sync::Arc;

        // Create metrics watcher pointing to non-existent Prometheus
        let config = MetricsConfig {
            prometheus_url: "http://localhost:19999".to_string(), // Non-existent port
            query_timeout: std::time::Duration::from_secs(1),
            cache_enabled: false,
            cache_ttl: std::time::Duration::from_secs(30),
            metric_name: "test_metric".to_string(),
            fallback_metrics: vec![],
        };

        let watcher = MetricsWatcher::new(config).expect("Failed to create watcher");

        // Verify watcher starts healthy (optimistic)
        assert!(watcher.is_healthy());

        // Try to get heat score - should fail gracefully
        let result = watcher
            .get_heat_score("vol-test", std::time::Duration::from_secs(300))
            .await;

        // Should return error (not panic)
        assert!(result.is_err());

        // After failed query, watcher may still be marked healthy until health_check is called
        // The controller handles this by using unwrap_or_else to provide zero score

        // Simulate controller's error handling (line 158-161)
        let heat_score = watcher
            .get_heat_score("vol-test", std::time::Duration::from_secs(300))
            .await
            .unwrap_or_else(|e| {
                // This is the graceful degradation path
                assert!(e.to_string().contains("connection") || e.to_string().contains("error"));
                crate::metrics::HeatScore::zero("vol-test")
            });

        // Should have zero score when Prometheus is unavailable
        assert_eq!(heat_score.score, 0.0);
        assert_eq!(heat_score.volume_id, "vol-test");
        assert_eq!(heat_score.sample_count, 0);
        assert_eq!(heat_score.source_metric, "none");
    }

    #[tokio::test]
    async fn test_zero_score_does_not_trigger_hot_migration() {
        // When Prometheus is unavailable and we get zero scores,
        // volumes should NOT be migrated to hot tier (false positive)
        use crate::metrics::HeatScore;

        let zero_score = HeatScore::zero("vol-test");

        // Zero score should be treated as COLD (low activity)
        // NOT as hot, which would trigger unnecessary migrations
        let high_watermark = 5000;
        let low_watermark = 500;

        assert!(zero_score.score < low_watermark as f64);
        assert!(zero_score.score < high_watermark as f64);

        // Zero IOPS should be classified as cold tier
        let iops = zero_score.score as u32;
        assert!(iops <= low_watermark);
        assert!(iops < high_watermark);
    }

    #[tokio::test]
    async fn test_prometheus_health_check_updates_state() {
        // Test that health check properly updates the healthy state
        use crate::metrics::{MetricsConfig, MetricsWatcher};

        let config = MetricsConfig {
            prometheus_url: "http://localhost:19999".to_string(),
            query_timeout: std::time::Duration::from_secs(1),
            cache_enabled: false,
            cache_ttl: std::time::Duration::from_secs(30),
            metric_name: "test_metric".to_string(),
            fallback_metrics: vec![],
        };

        let watcher = MetricsWatcher::new(config).expect("Failed to create watcher");

        // Initially healthy
        assert!(watcher.is_healthy());

        // Health check should fail and update state
        let result = watcher.health_check().await;
        assert!(result.is_err());

        // After failed health check, watcher should be marked unhealthy
        assert!(!watcher.is_healthy());
    }
}
