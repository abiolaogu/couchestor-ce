//! Custom Resource Definitions
//!
//! This module contains all CRD definitions used by the operator.

mod erasure_coding;
mod mayastor;
mod storage_policy;

// Re-export all types for public API
#[allow(unused_imports)]
pub use mayastor::{
    DiskPool, DiskPoolSpec, DiskPoolStatus, MayastorVolume, MayastorVolumeSpec,
    MayastorVolumeStatus, PoolState, ReplicaState, ReplicaStatus, VolumeState,
};

#[allow(unused_imports)]
pub use storage_policy::{
    parse_duration, ConditionStatus, LabelSelector, LabelSelectorOperator,
    LabelSelectorRequirement, MigrationHistoryEntry, PolicyCondition, PolicyPhase, StoragePolicy,
    StoragePolicySpec, StoragePolicyStatus,
};

#[allow(unused_imports)]
pub use erasure_coding::{
    ECStripe, ECStripeSpec, ECStripeStatus, EcAlgorithm, EcPolicyPhase, ErasureCodingPolicy,
    ErasureCodingPolicySpec, ErasureCodingPolicyStatus, JournalConfig, LbaRange, ShardHealth,
    ShardLocation, ShardState, StripeState,
};
