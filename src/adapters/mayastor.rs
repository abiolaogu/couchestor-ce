//! Mayastor Volume Adapter
//!
//! Implements the `VolumeManager` port using Mayastor Kubernetes CRDs.

use std::time::Duration;

use async_trait::async_trait;
use kube::api::{Api, Patch, PatchParams};
use kube::Client;
use serde_json::json;
use tokio::time::{sleep, Instant};
use tracing::{debug, info, instrument, warn};

use crate::crd::{
    MayastorVolume, ReplicaState as MayastorReplicaState, VolumeState as MayastorVolumeState,
};
use crate::domain::ports::{
    ReplicaInfo, ReplicaState, StorageTier, VolumeId, VolumeInfo, VolumeManager,
};
use crate::error::{Error, Result};

/// Mayastor-based volume manager adapter.
///
/// Implements the `VolumeManager` port using Mayastor Kubernetes CRDs.
#[allow(dead_code)]
#[derive(Clone)]
pub struct MayastorVolumeAdapter {
    client: Client,
    namespace: String,
}

#[allow(dead_code)]
impl MayastorVolumeAdapter {
    /// Create a new Mayastor volume adapter.
    pub fn new(client: Client, namespace: impl Into<String>) -> Self {
        Self {
            client,
            namespace: namespace.into(),
        }
    }

    /// Get the Kubernetes API for MayastorVolumes.
    fn volumes_api(&self) -> Api<MayastorVolume> {
        Api::namespaced(self.client.clone(), &self.namespace)
    }

    /// Convert Mayastor replica state to domain replica state.
    fn convert_replica_state(state: &MayastorReplicaState) -> ReplicaState {
        match state {
            MayastorReplicaState::Online => ReplicaState::Online,
            MayastorReplicaState::Degraded => ReplicaState::Degraded,
            MayastorReplicaState::Faulted => ReplicaState::Faulted,
            MayastorReplicaState::Unknown => ReplicaState::Unknown,
        }
    }

    /// Convert Mayastor volume to domain VolumeInfo.
    fn convert_volume(volume: &MayastorVolume) -> Option<VolumeInfo> {
        let volume_id = volume.metadata.name.as_ref()?;
        let status = volume.status.as_ref()?;

        let replicas: Vec<ReplicaInfo> = status
            .replicas
            .iter()
            .map(|r| ReplicaInfo {
                id: r.uuid.clone(),
                pool: r.pool.clone(),
                state: Self::convert_replica_state(&r.state),
                is_online: r.state == MayastorReplicaState::Online,
                is_synced: r.synced,
            })
            .collect();

        let is_healthy = status.state == MayastorVolumeState::Online
            && replicas.iter().all(|r| r.is_online && r.is_synced);

        // Determine tier from labels or default to Hot
        let tier = volume
            .metadata
            .labels
            .as_ref()
            .and_then(|l| l.get("storage-tier"))
            .map(|t| match t.as_str() {
                "cold" => StorageTier::Cold,
                "warm" => StorageTier::Warm,
                _ => StorageTier::Hot,
            })
            .unwrap_or(StorageTier::Hot);

        Some(VolumeInfo {
            id: VolumeId(volume_id.clone()),
            size_bytes: volume.spec.size,
            replicas,
            tier,
            is_healthy,
        })
    }
}

impl std::fmt::Debug for MayastorVolumeAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MayastorVolumeAdapter")
            .field("namespace", &self.namespace)
            .finish()
    }
}

#[async_trait]
impl VolumeManager for MayastorVolumeAdapter {
    #[instrument(skip(self))]
    async fn get_volume(&self, volume_id: &VolumeId) -> Result<Option<VolumeInfo>> {
        let api = self.volumes_api();

        match api.get(&volume_id.0).await {
            Ok(volume) => Ok(Self::convert_volume(&volume)),
            Err(kube::Error::Api(e)) if e.code == 404 => Ok(None),
            Err(e) => Err(Error::Kube(e)),
        }
    }

    #[instrument(skip(self))]
    async fn list_volumes(&self) -> Result<Vec<VolumeInfo>> {
        let api = self.volumes_api();
        let volumes = api.list(&Default::default()).await?;

        let infos: Vec<VolumeInfo> = volumes
            .items
            .iter()
            .filter_map(Self::convert_volume)
            .collect();

        Ok(infos)
    }

    #[instrument(skip(self))]
    async fn add_replica(&self, volume_id: &VolumeId, pool: &str) -> Result<ReplicaInfo> {
        let api = self.volumes_api();

        // Get current volume
        let volume = api.get(&volume_id.0).await?;
        let current_replicas = volume.spec.num_replicas;

        // Patch to increase replica count
        // Note: Mayastor handles replica placement, we specify topology
        let patch = json!({
            "spec": {
                "numReplicas": current_replicas + 1,
                "topology": {
                    "pool": {
                        "labelled": {
                            "inclusion": {
                                "pool": pool
                            }
                        }
                    }
                }
            }
        });

        let params = PatchParams::apply("couchestor");
        api.patch(&volume_id.0, &params, &Patch::Merge(&patch))
            .await?;

        info!(volume = %volume_id.0, pool = %pool, "Added replica request");

        // Return a pending replica info - actual UUID assigned by Mayastor
        Ok(ReplicaInfo {
            id: format!("pending-{}", pool),
            pool: pool.to_string(),
            state: ReplicaState::Unknown,
            is_online: false,
            is_synced: false,
        })
    }

    #[instrument(skip(self))]
    async fn remove_replica(&self, volume_id: &VolumeId, replica_id: &str) -> Result<()> {
        let api = self.volumes_api();

        // Get current volume
        let volume = api.get(&volume_id.0).await?;
        let current_replicas = volume.spec.num_replicas;

        if current_replicas <= 1 {
            return Err(Error::Internal("Cannot remove last replica".to_string()));
        }

        // Patch to decrease replica count
        let patch = json!({
            "spec": {
                "numReplicas": current_replicas - 1
            }
        });

        let params = PatchParams::apply("couchestor");
        api.patch(&volume_id.0, &params, &Patch::Merge(&patch))
            .await?;

        info!(
            volume = %volume_id.0,
            replica = %replica_id,
            "Removed replica"
        );

        Ok(())
    }

    #[instrument(skip(self))]
    async fn wait_replica_sync(
        &self,
        volume_id: &VolumeId,
        replica_id: &str,
        timeout: Duration,
    ) -> Result<bool> {
        let api = self.volumes_api();
        let start = Instant::now();
        let poll_interval = Duration::from_secs(2);

        while start.elapsed() < timeout {
            let volume = api.get(&volume_id.0).await?;

            if let Some(status) = &volume.status {
                // Find the replica
                let replica = status
                    .replicas
                    .iter()
                    .find(|r| r.uuid == replica_id || r.pool.contains(replica_id));

                if let Some(r) = replica {
                    if r.is_synced() {
                        debug!(
                            volume = %volume_id.0,
                            replica = %replica_id,
                            "Replica synced"
                        );
                        return Ok(true);
                    }
                    debug!(
                        volume = %volume_id.0,
                        replica = %replica_id,
                        state = ?r.state,
                        synced = r.synced,
                        "Waiting for replica sync"
                    );
                }
            }

            sleep(poll_interval).await;
        }

        warn!(
            volume = %volume_id.0,
            replica = %replica_id,
            elapsed = ?start.elapsed(),
            "Replica sync timeout"
        );

        Ok(false)
    }

    #[instrument(skip(self))]
    async fn get_volume_tier(&self, volume_id: &VolumeId) -> Result<StorageTier> {
        let volume = self.get_volume(volume_id).await?;
        Ok(volume.map(|v| v.tier).unwrap_or(StorageTier::Hot))
    }

    async fn health_check(&self) -> Result<bool> {
        // Try to list volumes as a health check
        let api = self.volumes_api();
        api.list(&Default::default()).await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::{MayastorVolumeSpec, MayastorVolumeStatus, ReplicaStatus};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    fn create_test_volume() -> MayastorVolume {
        MayastorVolume {
            metadata: ObjectMeta {
                name: Some("test-volume".to_string()),
                labels: Some([("storage-tier".to_string(), "hot".to_string())].into()),
                ..Default::default()
            },
            spec: MayastorVolumeSpec {
                num_replicas: 3,
                size: 1024 * 1024 * 1024, // 1GB
                topology: None,
            },
            status: Some(MayastorVolumeStatus {
                state: MayastorVolumeState::Online,
                replicas: vec![
                    ReplicaStatus {
                        uuid: "replica-1".to_string(),
                        pool: "pool-1".to_string(),
                        node: "node-1".to_string(),
                        state: MayastorReplicaState::Online,
                        synced: true,
                    },
                    ReplicaStatus {
                        uuid: "replica-2".to_string(),
                        pool: "pool-2".to_string(),
                        node: "node-2".to_string(),
                        state: MayastorReplicaState::Online,
                        synced: true,
                    },
                ],
                nexus: None,
            }),
        }
    }

    #[test]
    fn test_convert_volume() {
        let msv = create_test_volume();
        let info = MayastorVolumeAdapter::convert_volume(&msv).unwrap();

        assert_eq!(info.id.0, "test-volume");
        assert_eq!(info.size_bytes, 1024 * 1024 * 1024);
        assert_eq!(info.replicas.len(), 2);
        assert!(info.is_healthy);
        assert_eq!(info.tier, StorageTier::Hot);
    }

    #[test]
    fn test_convert_replica_state() {
        assert_eq!(
            MayastorVolumeAdapter::convert_replica_state(&MayastorReplicaState::Online),
            ReplicaState::Online
        );
        assert_eq!(
            MayastorVolumeAdapter::convert_replica_state(&MayastorReplicaState::Degraded),
            ReplicaState::Degraded
        );
    }
}
