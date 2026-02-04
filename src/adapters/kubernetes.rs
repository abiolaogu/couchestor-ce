//! Kubernetes Stripe Repository Adapter
//!
//! Implements the `StripeRepository` port using Kubernetes ECStripe CRDs.

use async_trait::async_trait;
use chrono::Utc;
use kube::api::{Api, ListParams, Patch, PatchParams, PostParams};
use kube::Client;
use tracing::{debug, instrument};

use crate::crd::{
    ECStripe, ECStripeSpec, LbaRange as CrdLbaRange, ShardLocation as CrdShardLocation,
};
use crate::domain::ports::{
    LbaRange, ShardLocation, StripeId, StripeMetadata, StripeRepository, VolumeId,
};
use crate::error::{Error, Result};

/// Kubernetes-based stripe repository adapter.
///
/// Persists stripe metadata as ECStripe custom resources in Kubernetes.
#[allow(dead_code)]
#[derive(Clone)]
pub struct KubernetesStripeRepository {
    client: Client,
    policy_ref: String,
}

#[allow(dead_code)]
impl KubernetesStripeRepository {
    /// Create a new Kubernetes stripe repository.
    pub fn new(client: Client, policy_ref: impl Into<String>) -> Self {
        Self {
            client,
            policy_ref: policy_ref.into(),
        }
    }

    /// Get the Kubernetes API for ECStripes.
    fn stripes_api(&self) -> Api<ECStripe> {
        Api::all(self.client.clone())
    }

    /// Generate a Kubernetes resource name for a stripe.
    fn stripe_name(volume_id: &VolumeId, stripe_id: &StripeId) -> String {
        format!("{}-stripe-{}", volume_id.0, stripe_id.0)
    }

    /// Convert domain StripeMetadata to CRD spec.
    fn to_crd_spec(&self, stripe: &StripeMetadata) -> ECStripeSpec {
        let shard_locations: Vec<CrdShardLocation> = stripe
            .shard_locations
            .iter()
            .map(|s| CrdShardLocation {
                shard_index: s.shard_index as u8,
                is_data_shard: true, // TODO: track this properly
                pool_name: s.device_id.clone(),
                node_name: s.device_id.clone(),
                offset: s.offset,
                size_bytes: s.size,
                checksum: None,
            })
            .collect();

        ECStripeSpec {
            volume_ref: stripe.volume_id.0.clone(),
            stripe_id: stripe.stripe_id.0,
            policy_ref: self.policy_ref.clone(),
            shard_locations,
            lba_range: CrdLbaRange {
                start_lba: stripe.lba_range.start,
                end_lba: stripe.lba_range.end,
            },
            checksum: None,
            generation: stripe.generation,
        }
    }

    /// Convert CRD to domain StripeMetadata.
    fn from_crd(stripe: &ECStripe) -> Option<StripeMetadata> {
        let spec = &stripe.spec;

        let shard_locations: Vec<ShardLocation> = spec
            .shard_locations
            .iter()
            .map(|s| ShardLocation {
                shard_index: s.shard_index as usize,
                device_id: s.pool_name.clone(),
                offset: s.offset,
                size: s.size_bytes,
            })
            .collect();

        Some(StripeMetadata {
            stripe_id: StripeId(spec.stripe_id),
            volume_id: VolumeId(spec.volume_ref.clone()),
            lba_range: LbaRange {
                start: spec.lba_range.start_lba,
                end: spec.lba_range.end_lba,
            },
            shard_locations,
            generation: spec.generation,
            created_at: stripe
                .metadata
                .creation_timestamp
                .as_ref()
                .map(|t| t.0)
                .unwrap_or_else(Utc::now),
            is_compressed: false, // TODO: track from status
            original_size: None,
        })
    }
}

impl std::fmt::Debug for KubernetesStripeRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KubernetesStripeRepository")
            .field("policy_ref", &self.policy_ref)
            .finish()
    }
}

#[async_trait]
impl StripeRepository for KubernetesStripeRepository {
    #[instrument(skip(self, stripe))]
    async fn save(&self, stripe: &StripeMetadata) -> Result<()> {
        let api = self.stripes_api();
        let name = Self::stripe_name(&stripe.volume_id, &stripe.stripe_id);

        let spec = self.to_crd_spec(stripe);

        // Check if exists
        match api.get(&name).await {
            Ok(_) => {
                // Update existing
                let patch = serde_json::json!({ "spec": spec });
                let params = PatchParams::apply("couchestor");
                api.patch(&name, &params, &Patch::Merge(&patch)).await?;
                debug!(name = %name, "Updated ECStripe");
            }
            Err(kube::Error::Api(e)) if e.code == 404 => {
                // Create new
                let mut ec_stripe = ECStripe::new(&name, spec);
                ec_stripe.metadata.labels = Some(
                    [
                        ("volume".to_string(), stripe.volume_id.0.clone()),
                        ("stripe-id".to_string(), stripe.stripe_id.0.to_string()),
                    ]
                    .into(),
                );

                api.create(&PostParams::default(), &ec_stripe).await?;
                debug!(name = %name, "Created ECStripe");
            }
            Err(e) => return Err(Error::Kube(e)),
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn find_by_id(&self, stripe_id: &StripeId) -> Result<Option<StripeMetadata>> {
        let api = self.stripes_api();

        // Search by stripe-id label
        let params = ListParams::default().labels(&format!("stripe-id={}", stripe_id.0));
        let list = api.list(&params).await?;

        Ok(list.items.first().and_then(Self::from_crd))
    }

    #[instrument(skip(self))]
    async fn find_by_lba(&self, volume_id: &VolumeId, lba: u64) -> Result<Option<StripeMetadata>> {
        // Get all stripes for the volume and filter by LBA
        let stripes = self.find_by_volume(volume_id).await?;

        Ok(stripes
            .into_iter()
            .find(|s| lba >= s.lba_range.start && lba < s.lba_range.end))
    }

    #[instrument(skip(self))]
    async fn find_by_volume(&self, volume_id: &VolumeId) -> Result<Vec<StripeMetadata>> {
        let api = self.stripes_api();

        let params = ListParams::default().labels(&format!("volume={}", volume_id.0));
        let list = api.list(&params).await?;

        Ok(list.items.iter().filter_map(Self::from_crd).collect())
    }

    #[instrument(skip(self))]
    async fn find_by_lba_range(
        &self,
        volume_id: &VolumeId,
        range: &LbaRange,
    ) -> Result<Vec<StripeMetadata>> {
        // Get all stripes for the volume and filter by LBA range
        let stripes = self.find_by_volume(volume_id).await?;

        Ok(stripes
            .into_iter()
            .filter(|s| {
                // Check if ranges overlap
                s.lba_range.start < range.end && range.start < s.lba_range.end
            })
            .collect())
    }

    #[instrument(skip(self))]
    async fn delete(&self, stripe_id: &StripeId) -> Result<()> {
        let api = self.stripes_api();

        // Find the stripe first
        let stripe = self.find_by_id(stripe_id).await?;
        if let Some(s) = stripe {
            let name = Self::stripe_name(&s.volume_id, stripe_id);
            api.delete(&name, &Default::default()).await?;
            debug!(name = %name, "Deleted ECStripe");
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn delete_by_volume(&self, volume_id: &VolumeId) -> Result<u64> {
        let api = self.stripes_api();

        let params = ListParams::default().labels(&format!("volume={}", volume_id.0));
        let list = api.list(&params).await?;

        let mut deleted = 0u64;
        for stripe in &list.items {
            if let Some(name) = &stripe.metadata.name {
                api.delete(name, &Default::default()).await?;
                deleted += 1;
            }
        }

        debug!(volume = %volume_id.0, deleted = deleted, "Deleted volume stripes");
        Ok(deleted)
    }

    #[instrument(skip(self))]
    async fn update_generation(&self, stripe_id: &StripeId, new_generation: u64) -> Result<bool> {
        let api = self.stripes_api();

        // Find the stripe first
        let stripe = self.find_by_id(stripe_id).await?;
        if let Some(s) = stripe {
            let name = Self::stripe_name(&s.volume_id, stripe_id);

            let patch = serde_json::json!({
                "spec": {
                    "generation": new_generation
                }
            });
            let params = PatchParams::apply("couchestor");
            api.patch(&name, &params, &Patch::Merge(&patch)).await?;

            debug!(name = %name, generation = new_generation, "Updated generation");
            Ok(true)
        } else {
            Ok(false)
        }
    }

    #[instrument(skip(self))]
    async fn count_by_volume(&self, volume_id: &VolumeId) -> Result<u64> {
        let stripes = self.find_by_volume(volume_id).await?;
        Ok(stripes.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stripe_name_generation() {
        let volume_id = VolumeId("test-volume".to_string());
        let stripe_id = StripeId(42);

        let name = KubernetesStripeRepository::stripe_name(&volume_id, &stripe_id);
        assert_eq!(name, "test-volume-stripe-42");
    }

    #[test]
    fn test_lba_range_conversion() {
        let domain_range = LbaRange {
            start: 100,
            end: 200,
        };

        let crd_range = CrdLbaRange {
            start_lba: 100,
            end_lba: 200,
        };

        assert_eq!(crd_range.start_lba, domain_range.start);
        assert_eq!(crd_range.end_lba, domain_range.end);
    }
}
