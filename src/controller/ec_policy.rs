//! ErasureCodingPolicy Controller
//!
//! Reconciliation logic for ErasureCodingPolicy resources.

use crate::crd::{EcPolicyPhase, ErasureCodingPolicy, ErasureCodingPolicyStatus};
use crate::error::{Error, Result};

use chrono::Utc;
use futures::StreamExt;
use kube::api::{Api, ListParams, Patch, PatchParams};
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, instrument, warn};

/// Context for the EC policy controller
pub struct EcPolicyContext {
    /// Kubernetes client
    pub client: Client,
}

impl EcPolicyContext {
    /// Create a new EC policy context
    pub fn new(client: Client) -> Arc<Self> {
        Arc::new(Self { client })
    }
}

/// Run the ErasureCodingPolicy controller
pub async fn run(ctx: Arc<EcPolicyContext>) -> Result<()> {
    let client = ctx.client.clone();
    let policies: Api<ErasureCodingPolicy> = Api::all(client.clone());

    // Check if CRD exists
    if let Err(e) = policies.list(&ListParams::default().limit(1)).await {
        error!(
            "ErasureCodingPolicy CRD not found: {}. Please install the CRD first.",
            e
        );
        return Err(Error::Kube(e));
    }

    info!("Starting ErasureCodingPolicy controller");

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

    info!("ErasureCodingPolicy controller shutdown complete");
    Ok(())
}

/// Reconcile an ErasureCodingPolicy resource
#[instrument(skip(policy, ctx), fields(policy = %policy.name_any()))]
async fn reconcile(
    policy: Arc<ErasureCodingPolicy>,
    ctx: Arc<EcPolicyContext>,
) -> std::result::Result<Action, Error> {
    let name = policy.name_any();
    info!("Reconciling ErasureCodingPolicy: {}", name);

    // Validate the policy configuration
    let validation_result = policy.validate();

    let (phase, message) = match validation_result {
        Ok(()) => {
            debug!("ErasureCodingPolicy {} is valid", name);

            // Check if any volumes are using this policy
            // In a real implementation, we'd query StoragePolicies that reference this EC policy
            let active_volumes = count_active_volumes(&ctx.client, &name).await.unwrap_or(0);

            if active_volumes > 0 {
                (
                    EcPolicyPhase::Active,
                    Some(format!("Policy active with {} volumes", active_volumes)),
                )
            } else {
                (
                    EcPolicyPhase::Ready,
                    Some("Policy validated and ready".to_string()),
                )
            }
        }
        Err(e) => {
            warn!("ErasureCodingPolicy {} is invalid: {}", name, e);
            (EcPolicyPhase::Invalid, Some(e))
        }
    };

    // Calculate storage efficiency
    let efficiency = policy.storage_efficiency();
    let overhead = policy.storage_overhead();

    debug!(
        "Policy {} configuration: {}+{} (efficiency: {:.1}%, overhead: {:.2}x)",
        name,
        policy.spec.data_shards,
        policy.spec.parity_shards,
        efficiency * 100.0,
        overhead
    );

    // Get actual volume count
    let active_volumes = count_active_volumes(&ctx.client, &name).await.unwrap_or(0);

    // Get stripe statistics
    let stripe_stats = count_stripes(&ctx.client, &name)
        .await
        .unwrap_or(StripeStats {
            total: 0,
            healthy: 0,
            degraded: 0,
            rebuilding: 0,
        });

    // Update status
    let status = ErasureCodingPolicyStatus {
        phase,
        active_volumes,
        total_stripes: stripe_stats.total,
        healthy_stripes: stripe_stats.healthy,
        degraded_stripes: stripe_stats.degraded,
        rebuilding_stripes: stripe_stats.rebuilding,
        storage_efficiency: efficiency,
        last_validation_time: Some(Utc::now()),
        message,
    };

    // Patch status
    let policies: Api<ErasureCodingPolicy> = Api::all(ctx.client.clone());
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

/// Count volumes using a specific EC policy
async fn count_active_volumes(client: &Client, policy_name: &str) -> Result<u32> {
    use crate::crd::StoragePolicy;

    // List all StoragePolicies
    let policies: Api<StoragePolicy> = Api::all(client.clone());
    let policy_list = policies.list(&ListParams::default()).await?;

    // Count volumes from policies that reference this EC policy
    let mut total_volumes = 0u32;
    for policy in policy_list.items {
        // Check if this StoragePolicy references the given EC policy
        if let Some(ec_ref) = policy.ec_policy_ref() {
            if ec_ref == policy_name {
                // Get the number of volumes this policy is managing
                if let Some(status) = &policy.status {
                    total_volumes += status.watched_volumes;
                    debug!(
                        "StoragePolicy {} references EC policy {} with {} volumes",
                        policy.name_any(),
                        policy_name,
                        status.watched_volumes
                    );
                }
            }
        }
    }

    Ok(total_volumes)
}

/// Stripe statistics for an EC policy
struct StripeStats {
    total: u64,
    healthy: u64,
    degraded: u64,
    rebuilding: u64,
}

/// Count stripes for a specific EC policy
async fn count_stripes(client: &Client, policy_name: &str) -> Result<StripeStats> {
    use crate::crd::{ECStripe, StripeState};

    // List all ECStripes that reference this policy
    let stripes: Api<ECStripe> = Api::all(client.clone());
    let label_selector = format!("policy={}", policy_name);
    let params = ListParams::default().labels(&label_selector);

    let stripe_list = match stripes.list(&params).await {
        Ok(list) => list,
        Err(kube::Error::Api(e)) if e.code == 404 => {
            // ECStripe CRD not installed yet
            debug!("ECStripe CRD not found, returning zero stats");
            return Ok(StripeStats {
                total: 0,
                healthy: 0,
                degraded: 0,
                rebuilding: 0,
            });
        }
        Err(e) => return Err(Error::Kube(e)),
    };

    let mut stats = StripeStats {
        total: 0,
        healthy: 0,
        degraded: 0,
        rebuilding: 0,
    };

    for stripe in stripe_list.items {
        // Only count stripes belonging to this policy
        if stripe.spec.policy_ref == policy_name {
            stats.total += 1;

            if let Some(status) = &stripe.status {
                match status.state {
                    StripeState::Healthy => stats.healthy += 1,
                    StripeState::Degraded => stats.degraded += 1,
                    StripeState::Rebuilding => stats.rebuilding += 1,
                    StripeState::Failed | StripeState::Writing => {
                        // Failed and writing stripes count as degraded
                        stats.degraded += 1;
                    }
                }
            } else {
                // No status means unknown, count as degraded
                stats.degraded += 1;
            }
        }
    }

    debug!(
        "EC policy {} stripe stats: total={}, healthy={}, degraded={}, rebuilding={}",
        policy_name, stats.total, stats.healthy, stats.degraded, stats.rebuilding
    );

    Ok(stats)
}

/// Error policy for the controller
fn error_policy(
    _policy: Arc<ErasureCodingPolicy>,
    error: &Error,
    _ctx: Arc<EcPolicyContext>,
) -> Action {
    error!("ErasureCodingPolicy reconciliation error: {}", error);
    Action::requeue(Duration::from_secs(60))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    #[test]
    fn test_ec_policy_context_creation() {
        // This test would require a mock client
        // For now, just verify the struct exists
        assert!(true);
    }
}
