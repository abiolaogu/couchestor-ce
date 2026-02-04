//! Controller module
//!
//! Implements the Kubernetes reconciliation loop for StoragePolicy and
//! ErasureCodingPolicy resources.

pub mod ec_policy;
mod storage_policy;

pub use ec_policy::{run as run_ec_policy, EcPolicyContext};
pub use storage_policy::{run, ControllerContext};
