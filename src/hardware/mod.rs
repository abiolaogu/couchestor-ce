//! Hardware Discovery Module
//!
//! Auto-detection of storage hardware including NVMe, SAS, and SATA devices.
//! Provides comprehensive hardware enumeration from Linux sysfs.
//!
//! # Features
//!
//! - Block device scanning via sysfs
//! - NVMe controller and namespace enumeration
//! - ZNS (Zoned Namespace) detection
//! - SMART data retrieval via nvme-cli and smartctl
//! - SAS/SATA device discovery
//!
//! # Example
//!
//! ```no_run
//! use couchestor::hardware::{HardwareScanner, ScannerConfig};
//!
//! # async fn example() -> couchestor::Result<()> {
//! let scanner = HardwareScanner::default_scanner();
//! let node_info = scanner.discover().await?;
//!
//! println!("Node: {}", node_info.hostname);
//! println!("Found {} drives", node_info.drives.len());
//!
//! for drive in &node_info.drives {
//!     println!("  {} - {} ({:?})",
//!         drive.device_path,
//!         drive.model,
//!         drive.drive_type
//!     );
//! }
//! # Ok(())
//! # }
//! ```

pub mod discovery;

pub use discovery::{
    scanner::{HardwareScanner, ScannerConfig},
    nvme::{NvmeControllerInfo, NvmeDiscovery, NvmeFeatures, NvmeNamespaceExtended, ZnsNamespaceInfo},
    sas_sata::{SasSataDiscovery, SataDeviceInfo},
    DriveInfo, DriveType, NodeHardwareInfo, NvmeNamespaceInfo, SmartData,
};
