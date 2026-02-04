//! Mayastor Custom Resource Definitions
//!
//! Mirrors the Mayastor CRDs for volume and pool management.

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// =============================================================================
// DiskPool CRD
// =============================================================================

/// DiskPool represents a Mayastor disk pool
#[derive(CustomResource, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "openebs.io",
    version = "v1beta2",
    kind = "DiskPool",
    plural = "diskpools",
    status = "DiskPoolStatus",
    namespaced = false
)]
#[serde(rename_all = "camelCase")]
pub struct DiskPoolSpec {
    /// Node on which the pool is located
    pub node: String,

    /// Disks that make up this pool
    pub disks: Vec<String>,
}

/// DiskPool status
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiskPoolStatus {
    /// Pool state
    #[serde(default)]
    pub state: PoolState,

    /// Available capacity in bytes
    #[serde(default)]
    pub available: u64,

    /// Used capacity in bytes
    #[serde(default)]
    pub used: u64,

    /// Total capacity in bytes
    #[serde(default)]
    pub capacity: u64,
}

/// Pool state
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum PoolState {
    #[default]
    Unknown,
    Online,
    Degraded,
    Faulted,
}

impl DiskPool {
    /// Get the pool name
    pub fn pool_name(&self) -> &str {
        self.metadata.name.as_deref().unwrap_or("unknown")
    }

    /// Get the pool labels
    pub fn labels(&self) -> BTreeMap<String, String> {
        self.metadata.labels.clone().unwrap_or_default()
    }

    /// Check if pool is online
    pub fn is_online(&self) -> bool {
        self.status
            .as_ref()
            .map(|s| s.state == PoolState::Online)
            .unwrap_or(false)
    }
}

// =============================================================================
// MayastorVolume CRD
// =============================================================================

/// MayastorVolume represents a Mayastor volume
#[derive(CustomResource, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "openebs.io",
    version = "v1alpha1",
    kind = "MayastorVolume",
    plural = "mayastorvolumes",
    shortname = "msv",
    status = "MayastorVolumeStatus",
    namespaced = true
)]
#[serde(rename_all = "camelCase")]
pub struct MayastorVolumeSpec {
    /// Number of replicas
    #[serde(default = "default_replicas")]
    pub num_replicas: u32,

    /// Size in bytes
    pub size: u64,

    /// Topology constraints
    #[serde(default)]
    pub topology: Option<VolumeTopology>,
}

fn default_replicas() -> u32 {
    1
}

/// Volume topology constraints
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VolumeTopology {
    /// Pool constraints
    #[serde(default)]
    pub pool: Option<PoolTopology>,
}

/// Pool topology constraints
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PoolTopology {
    /// Labelled pool selection
    #[serde(default)]
    pub labelled: Option<LabelledTopology>,
}

/// Labelled topology for pool selection
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LabelledTopology {
    /// Include pools with these labels
    #[serde(default)]
    pub inclusion: BTreeMap<String, String>,

    /// Exclude pools with these labels
    #[serde(default)]
    pub exclusion: BTreeMap<String, String>,
}

/// MayastorVolume status
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MayastorVolumeStatus {
    /// Volume state
    #[serde(default)]
    pub state: VolumeState,

    /// Replicas
    #[serde(default)]
    pub replicas: Vec<ReplicaStatus>,

    /// Target nexus
    #[serde(default)]
    pub nexus: Option<NexusStatus>,
}

/// Volume state
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum VolumeState {
    #[default]
    Unknown,
    Online,
    Degraded,
    Faulted,
}

/// Replica status
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplicaStatus {
    /// Replica UUID
    pub uuid: String,

    /// Pool name
    pub pool: String,

    /// Node name
    pub node: String,

    /// Replica state
    #[serde(default)]
    pub state: ReplicaState,

    /// Is this replica synced
    #[serde(default)]
    pub synced: bool,
}

impl ReplicaStatus {
    /// Check if replica is fully synced and online
    pub fn is_synced(&self) -> bool {
        self.state == ReplicaState::Online && self.synced
    }
}

/// Replica state
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum ReplicaState {
    #[default]
    Unknown,
    Online,
    Degraded,
    Faulted,
}

impl std::fmt::Display for ReplicaState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplicaState::Unknown => write!(f, "Unknown"),
            ReplicaState::Online => write!(f, "Online"),
            ReplicaState::Degraded => write!(f, "Degraded"),
            ReplicaState::Faulted => write!(f, "Faulted"),
        }
    }
}

/// Nexus status
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NexusStatus {
    /// Nexus UUID
    pub uuid: String,

    /// Node hosting the nexus
    pub node: String,

    /// Nexus state
    #[serde(default)]
    pub state: NexusState,
}

/// Nexus state
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum NexusState {
    #[default]
    Unknown,
    Online,
    Degraded,
    Faulted,
}

impl MayastorVolume {
    /// Get volume replicas
    pub fn replicas(&self) -> Vec<ReplicaStatus> {
        self.status
            .as_ref()
            .map(|s| s.replicas.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // PoolState Tests
    // =========================================================================

    #[test]
    fn test_pool_state_default() {
        let state = PoolState::default();
        assert_eq!(state, PoolState::Unknown);
    }

    #[test]
    fn test_pool_state_equality() {
        assert_eq!(PoolState::Online, PoolState::Online);
        assert_ne!(PoolState::Online, PoolState::Degraded);
        assert_ne!(PoolState::Faulted, PoolState::Unknown);
    }

    #[test]
    fn test_pool_state_serializes() {
        assert_eq!(
            serde_json::to_string(&PoolState::Unknown).unwrap(),
            "\"Unknown\""
        );
        assert_eq!(
            serde_json::to_string(&PoolState::Online).unwrap(),
            "\"Online\""
        );
        assert_eq!(
            serde_json::to_string(&PoolState::Degraded).unwrap(),
            "\"Degraded\""
        );
        assert_eq!(
            serde_json::to_string(&PoolState::Faulted).unwrap(),
            "\"Faulted\""
        );
    }

    #[test]
    fn test_pool_state_deserializes() {
        let online: PoolState = serde_json::from_str("\"Online\"").unwrap();
        assert_eq!(online, PoolState::Online);

        let faulted: PoolState = serde_json::from_str("\"Faulted\"").unwrap();
        assert_eq!(faulted, PoolState::Faulted);
    }

    // =========================================================================
    // VolumeState Tests
    // =========================================================================

    #[test]
    fn test_volume_state_default() {
        let state = VolumeState::default();
        assert_eq!(state, VolumeState::Unknown);
    }

    #[test]
    fn test_volume_state_equality() {
        assert_eq!(VolumeState::Online, VolumeState::Online);
        assert_ne!(VolumeState::Online, VolumeState::Degraded);
    }

    #[test]
    fn test_volume_state_serializes() {
        assert_eq!(
            serde_json::to_string(&VolumeState::Unknown).unwrap(),
            "\"Unknown\""
        );
        assert_eq!(
            serde_json::to_string(&VolumeState::Online).unwrap(),
            "\"Online\""
        );
        assert_eq!(
            serde_json::to_string(&VolumeState::Degraded).unwrap(),
            "\"Degraded\""
        );
        assert_eq!(
            serde_json::to_string(&VolumeState::Faulted).unwrap(),
            "\"Faulted\""
        );
    }

    // =========================================================================
    // ReplicaState Tests
    // =========================================================================

    #[test]
    fn test_replica_state_default() {
        let state = ReplicaState::default();
        assert_eq!(state, ReplicaState::Unknown);
    }

    #[test]
    fn test_replica_state_equality() {
        assert_eq!(ReplicaState::Online, ReplicaState::Online);
        assert_ne!(ReplicaState::Online, ReplicaState::Faulted);
    }

    #[test]
    fn test_replica_state_display() {
        assert_eq!(format!("{}", ReplicaState::Unknown), "Unknown");
        assert_eq!(format!("{}", ReplicaState::Online), "Online");
        assert_eq!(format!("{}", ReplicaState::Degraded), "Degraded");
        assert_eq!(format!("{}", ReplicaState::Faulted), "Faulted");
    }

    #[test]
    fn test_replica_state_serializes() {
        assert_eq!(
            serde_json::to_string(&ReplicaState::Unknown).unwrap(),
            "\"Unknown\""
        );
        assert_eq!(
            serde_json::to_string(&ReplicaState::Online).unwrap(),
            "\"Online\""
        );
        assert_eq!(
            serde_json::to_string(&ReplicaState::Degraded).unwrap(),
            "\"Degraded\""
        );
        assert_eq!(
            serde_json::to_string(&ReplicaState::Faulted).unwrap(),
            "\"Faulted\""
        );
    }

    // =========================================================================
    // NexusState Tests
    // =========================================================================

    #[test]
    fn test_nexus_state_default() {
        let state = NexusState::default();
        assert_eq!(state, NexusState::Unknown);
    }

    #[test]
    fn test_nexus_state_equality() {
        assert_eq!(NexusState::Online, NexusState::Online);
        assert_ne!(NexusState::Online, NexusState::Degraded);
    }

    #[test]
    fn test_nexus_state_serializes() {
        assert_eq!(
            serde_json::to_string(&NexusState::Unknown).unwrap(),
            "\"Unknown\""
        );
        assert_eq!(
            serde_json::to_string(&NexusState::Online).unwrap(),
            "\"Online\""
        );
        assert_eq!(
            serde_json::to_string(&NexusState::Degraded).unwrap(),
            "\"Degraded\""
        );
        assert_eq!(
            serde_json::to_string(&NexusState::Faulted).unwrap(),
            "\"Faulted\""
        );
    }

    // =========================================================================
    // ReplicaStatus Tests
    // =========================================================================

    #[test]
    fn test_replica_status_is_synced_true() {
        let replica = ReplicaStatus {
            uuid: "replica-123".to_string(),
            pool: "pool-nvme".to_string(),
            node: "node-1".to_string(),
            state: ReplicaState::Online,
            synced: true,
        };

        assert!(replica.is_synced());
    }

    #[test]
    fn test_replica_status_is_synced_false_not_online() {
        let replica = ReplicaStatus {
            uuid: "replica-123".to_string(),
            pool: "pool-nvme".to_string(),
            node: "node-1".to_string(),
            state: ReplicaState::Degraded,
            synced: true,
        };

        assert!(!replica.is_synced());
    }

    #[test]
    fn test_replica_status_is_synced_false_not_synced() {
        let replica = ReplicaStatus {
            uuid: "replica-123".to_string(),
            pool: "pool-nvme".to_string(),
            node: "node-1".to_string(),
            state: ReplicaState::Online,
            synced: false,
        };

        assert!(!replica.is_synced());
    }

    #[test]
    fn test_replica_status_is_synced_false_both() {
        let replica = ReplicaStatus {
            uuid: "replica-123".to_string(),
            pool: "pool-nvme".to_string(),
            node: "node-1".to_string(),
            state: ReplicaState::Unknown,
            synced: false,
        };

        assert!(!replica.is_synced());
    }

    #[test]
    fn test_replica_status_serializes() {
        let replica = ReplicaStatus {
            uuid: "replica-abc".to_string(),
            pool: "pool-1".to_string(),
            node: "worker-1".to_string(),
            state: ReplicaState::Online,
            synced: true,
        };

        let json = serde_json::to_string(&replica).unwrap();

        assert!(json.contains("\"uuid\":\"replica-abc\""));
        assert!(json.contains("\"pool\":\"pool-1\""));
        assert!(json.contains("\"node\":\"worker-1\""));
        assert!(json.contains("\"state\":\"Online\""));
        assert!(json.contains("\"synced\":true"));
    }

    // =========================================================================
    // DiskPoolStatus Tests
    // =========================================================================

    #[test]
    fn test_disk_pool_status_default() {
        let status = DiskPoolStatus::default();

        assert_eq!(status.state, PoolState::Unknown);
        assert_eq!(status.available, 0);
        assert_eq!(status.used, 0);
        assert_eq!(status.capacity, 0);
    }

    #[test]
    fn test_disk_pool_status_serializes() {
        let status = DiskPoolStatus {
            state: PoolState::Online,
            available: 1000000000,
            used: 500000000,
            capacity: 1500000000,
        };

        let json = serde_json::to_string(&status).unwrap();

        assert!(json.contains("\"state\":\"Online\""));
        assert!(json.contains("\"available\":1000000000"));
        assert!(json.contains("\"used\":500000000"));
        assert!(json.contains("\"capacity\":1500000000"));
    }

    // =========================================================================
    // MayastorVolumeStatus Tests
    // =========================================================================

    #[test]
    fn test_mayastor_volume_status_default() {
        let status = MayastorVolumeStatus::default();

        assert_eq!(status.state, VolumeState::Unknown);
        assert!(status.replicas.is_empty());
        assert!(status.nexus.is_none());
    }

    #[test]
    fn test_mayastor_volume_status_with_replicas() {
        let status = MayastorVolumeStatus {
            state: VolumeState::Online,
            replicas: vec![
                ReplicaStatus {
                    uuid: "r1".to_string(),
                    pool: "pool-1".to_string(),
                    node: "node-1".to_string(),
                    state: ReplicaState::Online,
                    synced: true,
                },
                ReplicaStatus {
                    uuid: "r2".to_string(),
                    pool: "pool-2".to_string(),
                    node: "node-2".to_string(),
                    state: ReplicaState::Online,
                    synced: true,
                },
            ],
            nexus: None,
        };

        assert_eq!(status.replicas.len(), 2);
        assert_eq!(status.replicas[0].uuid, "r1");
        assert_eq!(status.replicas[1].uuid, "r2");
    }

    // =========================================================================
    // NexusStatus Tests
    // =========================================================================

    #[test]
    fn test_nexus_status_default() {
        let status = NexusStatus::default();

        assert_eq!(status.uuid, "");
        assert_eq!(status.node, "");
        assert_eq!(status.state, NexusState::Unknown);
    }

    #[test]
    fn test_nexus_status_serializes() {
        let status = NexusStatus {
            uuid: "nexus-123".to_string(),
            node: "node-1".to_string(),
            state: NexusState::Online,
        };

        let json = serde_json::to_string(&status).unwrap();

        assert!(json.contains("\"uuid\":\"nexus-123\""));
        assert!(json.contains("\"node\":\"node-1\""));
        assert!(json.contains("\"state\":\"Online\""));
    }

    // =========================================================================
    // VolumeTopology Tests
    // =========================================================================

    #[test]
    fn test_volume_topology_default() {
        let topology = VolumeTopology::default();
        assert!(topology.pool.is_none());
    }

    #[test]
    fn test_pool_topology_default() {
        let topology = PoolTopology::default();
        assert!(topology.labelled.is_none());
    }

    #[test]
    fn test_labelled_topology_default() {
        let topology = LabelledTopology::default();
        assert!(topology.inclusion.is_empty());
        assert!(topology.exclusion.is_empty());
    }

    #[test]
    fn test_labelled_topology_with_labels() {
        let mut inclusion = BTreeMap::new();
        inclusion.insert("tier".to_string(), "nvme".to_string());

        let mut exclusion = BTreeMap::new();
        exclusion.insert("deprecated".to_string(), "true".to_string());

        let topology = LabelledTopology {
            inclusion,
            exclusion,
        };

        assert_eq!(topology.inclusion.get("tier"), Some(&"nvme".to_string()));
        assert_eq!(
            topology.exclusion.get("deprecated"),
            Some(&"true".to_string())
        );
    }

    // =========================================================================
    // DiskPoolSpec Tests
    // =========================================================================

    #[test]
    fn test_disk_pool_spec_serializes() {
        let spec = DiskPoolSpec {
            node: "worker-1".to_string(),
            disks: vec!["/dev/sda".to_string(), "/dev/sdb".to_string()],
        };

        let json = serde_json::to_string(&spec).unwrap();

        assert!(json.contains("\"node\":\"worker-1\""));
        assert!(json.contains("/dev/sda"));
        assert!(json.contains("/dev/sdb"));
    }

    // =========================================================================
    // MayastorVolumeSpec Tests
    // =========================================================================

    #[test]
    fn test_mayastor_volume_spec_serializes() {
        let spec = MayastorVolumeSpec {
            num_replicas: 3,
            size: 10737418240, // 10 GiB
            topology: None,
        };

        let json = serde_json::to_string(&spec).unwrap();

        assert!(json.contains("\"numReplicas\":3"));
        assert!(json.contains("\"size\":10737418240"));
    }

    #[test]
    fn test_mayastor_volume_spec_default_replicas() {
        assert_eq!(default_replicas(), 1);
    }
}
