//! EC Metadata Management
//!
//! Manages erasure coding metadata including LBA-to-stripe mappings
//! and ECStripe CRD persistence to Kubernetes.

use crate::crd::{
    ECStripe, ECStripeSpec, ECStripeStatus, ErasureCodingPolicy, LbaRange, ShardHealth,
    ShardLocation, ShardState, StripeState,
};
use crate::error::{Error, Result};
use chrono::Utc;
use dashmap::DashMap;
use kube::api::{Api, ListParams, Patch, PatchParams, PostParams};
use kube::Client;
use std::collections::BTreeMap;
use std::sync::Arc;
use tracing::{debug, info, instrument};

// =============================================================================
// Stripe Metadata
// =============================================================================

/// Metadata for a single stripe (in-memory representation)
#[derive(Debug, Clone)]
pub struct StripeMetadata {
    /// Unique stripe ID within the volume
    pub stripe_id: u64,

    /// Volume this stripe belongs to
    pub volume_id: String,

    /// Policy reference
    pub policy_ref: String,

    /// LBA range covered by this stripe
    pub lba_range: LbaRange,

    /// Shard locations
    pub shard_locations: Vec<ShardLocation>,

    /// Current health status
    pub status: StripeStatus,

    /// Generation number
    pub generation: u64,

    /// Checksum
    pub checksum: Option<String>,
}

/// Status of a stripe in memory
#[derive(Debug, Clone, Default)]
pub struct StripeStatus {
    /// Current state
    pub state: StripeState,

    /// Number of healthy shards
    pub healthy_shards: u8,

    /// Individual shard health
    pub shard_health: Vec<ShardHealth>,
}

// =============================================================================
// Volume EC State
// =============================================================================

/// EC state for a single volume
#[derive(Debug)]
pub struct VolumeEcState {
    /// Volume ID
    pub volume_id: String,

    /// EC policy being used
    pub policy_ref: String,

    /// LBA to stripe mapping
    lba_map: BTreeMap<u64, u64>, // start_lba -> stripe_id

    /// Stripe metadata by ID
    stripes: DashMap<u64, StripeMetadata>,

    /// Next stripe ID
    next_stripe_id: std::sync::atomic::AtomicU64,

    /// Total data size
    pub data_size: u64,
}

impl VolumeEcState {
    /// Create a new volume EC state
    pub fn new(volume_id: String, policy_ref: String) -> Self {
        Self {
            volume_id,
            policy_ref,
            lba_map: BTreeMap::new(),
            stripes: DashMap::new(),
            next_stripe_id: std::sync::atomic::AtomicU64::new(0),
            data_size: 0,
        }
    }

    /// Get the next stripe ID
    pub fn next_stripe_id(&self) -> u64 {
        self.next_stripe_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// Add a stripe to the volume
    pub fn add_stripe(&mut self, metadata: StripeMetadata) {
        self.lba_map
            .insert(metadata.lba_range.start_lba, metadata.stripe_id);
        self.stripes.insert(metadata.stripe_id, metadata);
    }

    /// Find stripe containing an LBA
    pub fn find_stripe_for_lba(&self, lba: u64) -> Option<StripeMetadata> {
        // Find the stripe with the largest start_lba <= lba
        let stripe_id = self.lba_map.range(..=lba).next_back().map(|(_, id)| *id)?;

        self.stripes.get(&stripe_id).map(|s| s.clone())
    }

    /// Get all stripes in an LBA range
    pub fn find_stripes_in_range(&self, range: &LbaRange) -> Vec<StripeMetadata> {
        self.stripes
            .iter()
            .filter(|entry| entry.value().lba_range.overlaps(range))
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get a stripe by ID
    pub fn get_stripe(&self, stripe_id: u64) -> Option<StripeMetadata> {
        self.stripes.get(&stripe_id).map(|s| s.clone())
    }

    /// Update stripe status
    pub fn update_stripe_status(&self, stripe_id: u64, status: StripeStatus) {
        if let Some(mut stripe) = self.stripes.get_mut(&stripe_id) {
            stripe.status = status;
        }
    }

    /// Get count of stripes by state
    pub fn stripe_counts(&self) -> StripeStateCounts {
        let mut counts = StripeStateCounts::default();

        for entry in self.stripes.iter() {
            match entry.value().status.state {
                StripeState::Healthy => counts.healthy += 1,
                StripeState::Degraded => counts.degraded += 1,
                StripeState::Rebuilding => counts.rebuilding += 1,
                StripeState::Failed => counts.failed += 1,
                StripeState::Writing => counts.writing += 1,
            }
        }

        counts
    }

    /// Get all stripes
    pub fn all_stripes(&self) -> Vec<StripeMetadata> {
        self.stripes.iter().map(|e| e.value().clone()).collect()
    }

    /// Get total stripe count
    pub fn stripe_count(&self) -> usize {
        self.stripes.len()
    }
}

/// Counts of stripes by state
#[derive(Debug, Default)]
pub struct StripeStateCounts {
    pub healthy: u64,
    pub degraded: u64,
    pub rebuilding: u64,
    pub failed: u64,
    pub writing: u64,
}

// =============================================================================
// EC Metadata Manager
// =============================================================================

/// Manages EC metadata for all volumes
pub struct EcMetadataManager {
    /// Kubernetes client
    client: Client,

    /// Per-volume EC state
    volumes: DashMap<String, Arc<parking_lot::RwLock<VolumeEcState>>>,

    /// Cached EC policies
    policies: DashMap<String, ErasureCodingPolicy>,
}

impl EcMetadataManager {
    /// Create a new metadata manager
    pub fn new(client: Client) -> Arc<Self> {
        Arc::new(Self {
            client,
            volumes: DashMap::new(),
            policies: DashMap::new(),
        })
    }

    /// Get or create volume EC state
    pub fn get_or_create_volume(
        &self,
        volume_id: &str,
        policy_ref: &str,
    ) -> Arc<parking_lot::RwLock<VolumeEcState>> {
        self.volumes
            .entry(volume_id.to_string())
            .or_insert_with(|| {
                Arc::new(parking_lot::RwLock::new(VolumeEcState::new(
                    volume_id.to_string(),
                    policy_ref.to_string(),
                )))
            })
            .clone()
    }

    /// Get volume EC state if it exists
    pub fn get_volume(&self, volume_id: &str) -> Option<Arc<parking_lot::RwLock<VolumeEcState>>> {
        self.volumes.get(volume_id).map(|v| v.clone())
    }

    /// Check if a volume has EC enabled
    pub fn volume_has_ec(&self, volume_id: &str) -> bool {
        self.volumes.contains_key(volume_id)
    }

    /// Load EC policy from Kubernetes
    #[instrument(skip(self))]
    pub async fn load_policy(&self, policy_name: &str) -> Result<ErasureCodingPolicy> {
        // Check cache first
        if let Some(policy) = self.policies.get(policy_name) {
            return Ok(policy.clone());
        }

        // Load from Kubernetes
        let policies_api: Api<ErasureCodingPolicy> = Api::all(self.client.clone());
        let policy = policies_api
            .get(policy_name)
            .await
            .map_err(|_| Error::EcPolicyNotFound(policy_name.to_string()))?;

        // Cache it
        self.policies
            .insert(policy_name.to_string(), policy.clone());

        Ok(policy)
    }

    /// Refresh EC policy cache
    #[instrument(skip(self))]
    pub async fn refresh_policies(&self) -> Result<()> {
        let policies_api: Api<ErasureCodingPolicy> = Api::all(self.client.clone());
        let policies = policies_api.list(&ListParams::default()).await?;

        for policy in policies.items {
            if let Some(name) = &policy.metadata.name {
                self.policies.insert(name.clone(), policy);
            }
        }

        info!("Refreshed {} EC policies", self.policies.len());
        Ok(())
    }

    /// Create an ECStripe CRD in Kubernetes
    #[instrument(skip(self, metadata))]
    pub async fn create_stripe_crd(&self, metadata: &StripeMetadata) -> Result<ECStripe> {
        let stripes_api: Api<ECStripe> = Api::all(self.client.clone());

        // Generate CRD name
        let name = format!(
            "{}-stripe-{}",
            metadata.volume_id.replace('/', "-"),
            metadata.stripe_id
        );

        // Build the ECStripe spec
        let stripe = ECStripe::new(
            &name,
            ECStripeSpec {
                volume_ref: metadata.volume_id.clone(),
                stripe_id: metadata.stripe_id,
                policy_ref: metadata.policy_ref.clone(),
                shard_locations: metadata.shard_locations.clone(),
                lba_range: metadata.lba_range.clone(),
                checksum: metadata.checksum.clone(),
                generation: metadata.generation,
            },
        );

        // Create in Kubernetes
        let created = stripes_api
            .create(&PostParams::default(), &stripe)
            .await
            .map_err(|e| Error::Internal(format!("Failed to create ECStripe CRD: {}", e)))?;

        info!("Created ECStripe CRD: {}", name);
        Ok(created)
    }

    /// Update ECStripe CRD status
    #[instrument(skip(self))]
    pub async fn update_stripe_status(
        &self,
        stripe_name: &str,
        state: StripeState,
        healthy_shards: u8,
        shard_health: Vec<ShardHealth>,
    ) -> Result<()> {
        let stripes_api: Api<ECStripe> = Api::all(self.client.clone());

        let healthy_data = shard_health
            .iter()
            .filter(|h| h.state == ShardState::Healthy && h.shard_index < 128) // Assume first half are data
            .count() as u8;

        let healthy_parity = shard_health
            .iter()
            .filter(|h| h.state == ShardState::Healthy && h.shard_index >= 128)
            .count() as u8;

        let status = ECStripeStatus {
            state,
            healthy_shards,
            healthy_data_shards: healthy_data,
            healthy_parity_shards: healthy_parity,
            last_verification_time: Some(Utc::now()),
            last_modified_time: Some(Utc::now()),
            rebuild_progress: None,
            shard_health,
        };

        let patch = serde_json::json!({ "status": status });
        stripes_api
            .patch_status(
                stripe_name,
                &PatchParams::apply("smart-storage-operator"),
                &Patch::Merge(&patch),
            )
            .await
            .map_err(|e| Error::Internal(format!("Failed to update ECStripe status: {}", e)))?;

        debug!("Updated ECStripe status: {}", stripe_name);
        Ok(())
    }

    /// Load all ECStripe CRDs for a volume
    #[instrument(skip(self))]
    pub async fn load_volume_stripes(&self, volume_id: &str) -> Result<Vec<ECStripe>> {
        let stripes_api: Api<ECStripe> = Api::all(self.client.clone());

        // List all stripes and filter by volume
        // In a real implementation, we'd use a label selector
        let all_stripes = stripes_api.list(&ListParams::default()).await?;

        let volume_stripes: Vec<ECStripe> = all_stripes
            .items
            .into_iter()
            .filter(|s| s.spec.volume_ref == volume_id)
            .collect();

        debug!(
            "Loaded {} ECStripe CRDs for volume {}",
            volume_stripes.len(),
            volume_id
        );

        Ok(volume_stripes)
    }

    /// Delete an ECStripe CRD
    #[instrument(skip(self))]
    pub async fn delete_stripe_crd(&self, stripe_name: &str) -> Result<()> {
        let stripes_api: Api<ECStripe> = Api::all(self.client.clone());

        stripes_api
            .delete(stripe_name, &Default::default())
            .await
            .map_err(|e| Error::Internal(format!("Failed to delete ECStripe CRD: {}", e)))?;

        info!("Deleted ECStripe CRD: {}", stripe_name);
        Ok(())
    }

    /// Sync in-memory state from Kubernetes CRDs
    #[instrument(skip(self))]
    pub async fn sync_from_crds(&self, volume_id: &str, policy_ref: &str) -> Result<()> {
        let stripes = self.load_volume_stripes(volume_id).await?;

        let volume_state = self.get_or_create_volume(volume_id, policy_ref);
        let mut state = volume_state.write();

        for stripe in stripes {
            let metadata = StripeMetadata {
                stripe_id: stripe.spec.stripe_id,
                volume_id: stripe.spec.volume_ref.clone(),
                policy_ref: stripe.spec.policy_ref.clone(),
                lba_range: stripe.spec.lba_range.clone(),
                shard_locations: stripe.spec.shard_locations.clone(),
                status: StripeStatus {
                    state: stripe
                        .status
                        .as_ref()
                        .map(|s| s.state.clone())
                        .unwrap_or_default(),
                    healthy_shards: stripe
                        .status
                        .as_ref()
                        .map(|s| s.healthy_shards)
                        .unwrap_or(0),
                    shard_health: stripe
                        .status
                        .as_ref()
                        .map(|s| s.shard_health.clone())
                        .unwrap_or_default(),
                },
                generation: stripe.spec.generation,
                checksum: stripe.spec.checksum.clone(),
            };

            state.add_stripe(metadata);
        }

        info!(
            "Synced {} stripes for volume {} from CRDs",
            state.stripe_count(),
            volume_id
        );

        Ok(())
    }

    /// Get aggregate stats across all volumes
    pub fn aggregate_stats(&self) -> AggregateEcStats {
        let mut stats = AggregateEcStats::default();

        for entry in self.volumes.iter() {
            let state = entry.value().read();
            let counts = state.stripe_counts();

            stats.total_volumes += 1;
            stats.total_stripes += state.stripe_count() as u64;
            stats.healthy_stripes += counts.healthy;
            stats.degraded_stripes += counts.degraded;
            stats.rebuilding_stripes += counts.rebuilding;
            stats.failed_stripes += counts.failed;
        }

        stats
    }
}

/// Aggregate EC statistics
#[derive(Debug, Default)]
pub struct AggregateEcStats {
    pub total_volumes: u64,
    pub total_stripes: u64,
    pub healthy_stripes: u64,
    pub degraded_stripes: u64,
    pub rebuilding_stripes: u64,
    pub failed_stripes: u64,
}

// =============================================================================
// LBA Stripe Map
// =============================================================================

/// Fast LBA to stripe lookup using a B-tree
#[derive(Debug, Default)]
pub struct LbaStripeMap {
    /// Maps stripe start LBA to stripe ID
    map: BTreeMap<u64, u64>,
}

impl LbaStripeMap {
    /// Create a new LBA stripe map
    pub fn new() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    /// Insert a stripe range
    pub fn insert(&mut self, start_lba: u64, stripe_id: u64) {
        self.map.insert(start_lba, stripe_id);
    }

    /// Find the stripe containing an LBA
    pub fn find(&self, lba: u64) -> Option<u64> {
        self.map.range(..=lba).next_back().map(|(_, id)| *id)
    }

    /// Remove a stripe
    pub fn remove(&mut self, start_lba: u64) -> Option<u64> {
        self.map.remove(&start_lba)
    }

    /// Get all stripe IDs in a range
    pub fn range(&self, start: u64, end: u64) -> Vec<u64> {
        self.map.range(start..end).map(|(_, id)| *id).collect()
    }

    /// Get the number of entries
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::LbaRange;

    // =========================================================================
    // LbaStripeMap Tests
    // =========================================================================

    #[test]
    fn test_lba_stripe_map_new() {
        let map = LbaStripeMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
    }

    #[test]
    fn test_lba_stripe_map_insert_find() {
        let mut map = LbaStripeMap::new();

        map.insert(0, 1); // Stripe 1: 0-99
        map.insert(100, 2); // Stripe 2: 100-199
        map.insert(200, 3); // Stripe 3: 200-299

        assert_eq!(map.find(0), Some(1));
        assert_eq!(map.find(50), Some(1));
        assert_eq!(map.find(99), Some(1));
        assert_eq!(map.find(100), Some(2));
        assert_eq!(map.find(150), Some(2));
        assert_eq!(map.find(200), Some(3));
        assert_eq!(map.find(250), Some(3));
    }

    #[test]
    fn test_lba_stripe_map_range() {
        let mut map = LbaStripeMap::new();

        map.insert(0, 1);
        map.insert(100, 2);
        map.insert(200, 3);
        map.insert(300, 4);

        let range = map.range(50, 250);
        assert_eq!(range, vec![2, 3]);
    }

    #[test]
    fn test_lba_stripe_map_remove() {
        let mut map = LbaStripeMap::new();

        map.insert(0, 1);
        map.insert(100, 2);

        assert_eq!(map.remove(0), Some(1));
        assert_eq!(map.find(50), None);
        assert_eq!(map.find(100), Some(2));
    }

    // =========================================================================
    // VolumeEcState Tests
    // =========================================================================

    #[test]
    fn test_volume_ec_state_new() {
        let state = VolumeEcState::new("vol-1".to_string(), "policy-1".to_string());

        assert_eq!(state.volume_id, "vol-1");
        assert_eq!(state.policy_ref, "policy-1");
        assert_eq!(state.stripe_count(), 0);
    }

    #[test]
    fn test_volume_ec_state_add_stripe() {
        let mut state = VolumeEcState::new("vol-1".to_string(), "policy-1".to_string());

        let metadata = StripeMetadata {
            stripe_id: 0,
            volume_id: "vol-1".to_string(),
            policy_ref: "policy-1".to_string(),
            lba_range: LbaRange::new(0, 1000),
            shard_locations: vec![],
            status: StripeStatus::default(),
            generation: 0,
            checksum: None,
        };

        state.add_stripe(metadata);

        assert_eq!(state.stripe_count(), 1);
        assert!(state.get_stripe(0).is_some());
    }

    #[test]
    fn test_volume_ec_state_find_stripe_for_lba() {
        let mut state = VolumeEcState::new("vol-1".to_string(), "policy-1".to_string());

        // Add stripes
        state.add_stripe(StripeMetadata {
            stripe_id: 0,
            volume_id: "vol-1".to_string(),
            policy_ref: "policy-1".to_string(),
            lba_range: LbaRange::new(0, 1000),
            shard_locations: vec![],
            status: StripeStatus::default(),
            generation: 0,
            checksum: None,
        });

        state.add_stripe(StripeMetadata {
            stripe_id: 1,
            volume_id: "vol-1".to_string(),
            policy_ref: "policy-1".to_string(),
            lba_range: LbaRange::new(1000, 2000),
            shard_locations: vec![],
            status: StripeStatus::default(),
            generation: 0,
            checksum: None,
        });

        // Find stripes
        let stripe = state.find_stripe_for_lba(500);
        assert!(stripe.is_some());
        assert_eq!(stripe.unwrap().stripe_id, 0);

        let stripe = state.find_stripe_for_lba(1500);
        assert!(stripe.is_some());
        assert_eq!(stripe.unwrap().stripe_id, 1);
    }

    #[test]
    fn test_volume_ec_state_stripe_counts() {
        let mut state = VolumeEcState::new("vol-1".to_string(), "policy-1".to_string());

        // Add healthy stripe
        state.add_stripe(StripeMetadata {
            stripe_id: 0,
            volume_id: "vol-1".to_string(),
            policy_ref: "policy-1".to_string(),
            lba_range: LbaRange::new(0, 1000),
            shard_locations: vec![],
            status: StripeStatus {
                state: StripeState::Healthy,
                healthy_shards: 6,
                shard_health: vec![],
            },
            generation: 0,
            checksum: None,
        });

        // Add degraded stripe
        state.add_stripe(StripeMetadata {
            stripe_id: 1,
            volume_id: "vol-1".to_string(),
            policy_ref: "policy-1".to_string(),
            lba_range: LbaRange::new(1000, 2000),
            shard_locations: vec![],
            status: StripeStatus {
                state: StripeState::Degraded,
                healthy_shards: 4,
                shard_health: vec![],
            },
            generation: 0,
            checksum: None,
        });

        let counts = state.stripe_counts();
        assert_eq!(counts.healthy, 1);
        assert_eq!(counts.degraded, 1);
        assert_eq!(counts.failed, 0);
    }

    // =========================================================================
    // StripeStatus Tests
    // =========================================================================

    #[test]
    fn test_stripe_status_default() {
        let status = StripeStatus::default();
        assert_eq!(status.state, StripeState::Healthy);
        assert_eq!(status.healthy_shards, 0);
        assert!(status.shard_health.is_empty());
    }

    // =========================================================================
    // StripeMetadata Tests
    // =========================================================================

    #[test]
    fn test_stripe_metadata_creation() {
        let metadata = StripeMetadata {
            stripe_id: 42,
            volume_id: "vol-test".to_string(),
            policy_ref: "ec-policy".to_string(),
            lba_range: LbaRange::new(0, 1048576),
            shard_locations: vec![],
            status: StripeStatus::default(),
            generation: 1,
            checksum: Some("abc123".to_string()),
        };

        assert_eq!(metadata.stripe_id, 42);
        assert_eq!(metadata.lba_range.size(), 1048576);
        assert!(metadata.checksum.is_some());
    }
}
