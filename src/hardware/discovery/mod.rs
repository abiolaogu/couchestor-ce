//! Hardware Discovery Components
//!
//! Provides device enumeration and metadata collection for storage hardware.

pub mod nvme;
pub mod sas_sata;
pub mod scanner;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// =============================================================================
// Drive Types
// =============================================================================

/// Type of storage drive
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriveType {
    /// NVMe SSD
    Nvme,
    /// SATA/SAS SSD
    Ssd,
    /// Spinning HDD
    Hdd,
    /// Unknown device type
    Unknown,
}

impl DriveType {
    /// Get performance tier for this drive type
    pub fn performance_tier(&self) -> u8 {
        match self {
            DriveType::Nvme => 1,   // Highest performance
            DriveType::Ssd => 2,    // High performance
            DriveType::Hdd => 3,    // Standard
            DriveType::Unknown => 4, // Lowest priority
        }
    }

    /// Check if this is a solid-state device
    pub fn is_solid_state(&self) -> bool {
        matches!(self, DriveType::Nvme | DriveType::Ssd)
    }
}

impl std::fmt::Display for DriveType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriveType::Nvme => write!(f, "NVMe"),
            DriveType::Ssd => write!(f, "SSD"),
            DriveType::Hdd => write!(f, "HDD"),
            DriveType::Unknown => write!(f, "Unknown"),
        }
    }
}

// =============================================================================
// Drive Information
// =============================================================================

/// Information about a discovered storage drive
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveInfo {
    /// Device path (e.g., /dev/nvme0n1)
    pub device_path: String,
    /// Device identifier (e.g., nvme0n1)
    pub device_id: String,
    /// Type of drive
    pub drive_type: DriveType,
    /// Device model name
    pub model: String,
    /// Serial number
    pub serial: String,
    /// Firmware version
    pub firmware: String,
    /// Total capacity in bytes
    pub capacity_bytes: u64,
    /// Logical block size
    pub block_size: u32,
    /// Whether ZNS (Zoned Namespace) is supported
    pub zns_supported: bool,
    /// NVMe namespace information (if NVMe device)
    pub nvme_namespaces: Vec<NvmeNamespaceInfo>,
    /// SMART data (if available)
    pub smart_data: Option<SmartData>,
}

impl DriveInfo {
    /// Get capacity in GiB
    pub fn capacity_gib(&self) -> f64 {
        self.capacity_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
    }

    /// Check if drive is healthy based on SMART data
    pub fn is_healthy(&self) -> bool {
        if let Some(smart) = &self.smart_data {
            smart.critical_warning == 0 && smart.percentage_used < 90
        } else {
            true // Assume healthy if no SMART data
        }
    }
}

// =============================================================================
// NVMe Namespace Info
// =============================================================================

/// Information about an NVMe namespace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NvmeNamespaceInfo {
    /// Namespace ID
    pub nsid: u32,
    /// Capacity in bytes
    pub capacity_bytes: u64,
    /// Whether namespace is active
    pub active: bool,
    /// Whether this is a ZNS namespace
    pub is_zns: bool,
}

// =============================================================================
// SMART Data
// =============================================================================

/// SMART health data for a drive
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartData {
    /// Temperature in Celsius
    pub temperature_celsius: i32,
    /// Percentage of device lifetime used
    pub percentage_used: u8,
    /// Data units read (each unit is 512KB * 1000)
    pub data_units_read: u64,
    /// Data units written
    pub data_units_written: u64,
    /// Power-on hours
    pub power_on_hours: u64,
    /// Critical warning flags (0 = healthy)
    pub critical_warning: u8,
}

impl SmartData {
    /// Calculate total data read in TB
    pub fn data_read_tb(&self) -> f64 {
        (self.data_units_read * 512 * 1000) as f64 / (1024.0 * 1024.0 * 1024.0 * 1024.0)
    }

    /// Calculate total data written in TB
    pub fn data_written_tb(&self) -> f64 {
        (self.data_units_written * 512 * 1000) as f64 / (1024.0 * 1024.0 * 1024.0 * 1024.0)
    }

    /// Check if there are any critical warnings
    pub fn has_critical_warning(&self) -> bool {
        self.critical_warning != 0
    }

    /// Get estimated remaining life percentage
    pub fn remaining_life_percent(&self) -> u8 {
        100u8.saturating_sub(self.percentage_used)
    }
}

// =============================================================================
// Node Hardware Info
// =============================================================================

/// Aggregated hardware information for a node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHardwareInfo {
    /// Node identifier
    pub node_id: String,
    /// Node hostname
    pub hostname: String,
    /// Discovered drives
    pub drives: Vec<DriveInfo>,
    /// Total system memory in bytes
    pub memory_bytes: u64,
    /// CPU count
    pub cpu_count: u32,
    /// Discovery timestamp
    pub discovered_at: DateTime<Utc>,
}

impl NodeHardwareInfo {
    /// Get total storage capacity across all drives
    pub fn total_storage_bytes(&self) -> u64 {
        self.drives.iter().map(|d| d.capacity_bytes).sum()
    }

    /// Get drives by type
    pub fn drives_by_type(&self, drive_type: DriveType) -> Vec<&DriveInfo> {
        self.drives.iter().filter(|d| d.drive_type == drive_type).collect()
    }

    /// Get NVMe drives only
    pub fn nvme_drives(&self) -> Vec<&DriveInfo> {
        self.drives_by_type(DriveType::Nvme)
    }

    /// Get SSD drives (NVMe + SATA SSD)
    pub fn ssd_drives(&self) -> Vec<&DriveInfo> {
        self.drives.iter().filter(|d| d.drive_type.is_solid_state()).collect()
    }

    /// Get HDD drives only
    pub fn hdd_drives(&self) -> Vec<&DriveInfo> {
        self.drives_by_type(DriveType::Hdd)
    }

    /// Get drives that support ZNS
    pub fn zns_drives(&self) -> Vec<&DriveInfo> {
        self.drives.iter().filter(|d| d.zns_supported).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drive_type_performance_tier() {
        assert_eq!(DriveType::Nvme.performance_tier(), 1);
        assert_eq!(DriveType::Ssd.performance_tier(), 2);
        assert_eq!(DriveType::Hdd.performance_tier(), 3);
        assert_eq!(DriveType::Unknown.performance_tier(), 4);
    }

    #[test]
    fn test_drive_type_is_solid_state() {
        assert!(DriveType::Nvme.is_solid_state());
        assert!(DriveType::Ssd.is_solid_state());
        assert!(!DriveType::Hdd.is_solid_state());
        assert!(!DriveType::Unknown.is_solid_state());
    }

    #[test]
    fn test_smart_data_calculations() {
        let smart = SmartData {
            temperature_celsius: 45,
            percentage_used: 10,
            data_units_read: 1000000,
            data_units_written: 500000,
            power_on_hours: 8760,
            critical_warning: 0,
        };

        assert_eq!(smart.remaining_life_percent(), 90);
        assert!(!smart.has_critical_warning());
        assert!(smart.data_read_tb() > 0.0);
        assert!(smart.data_written_tb() > 0.0);
    }

    #[test]
    fn test_drive_info_capacity_gib() {
        let drive = DriveInfo {
            device_path: "/dev/nvme0n1".to_string(),
            device_id: "nvme0n1".to_string(),
            drive_type: DriveType::Nvme,
            model: "Test Drive".to_string(),
            serial: "12345".to_string(),
            firmware: "1.0".to_string(),
            capacity_bytes: 1024 * 1024 * 1024 * 1024, // 1 TiB
            block_size: 512,
            zns_supported: false,
            nvme_namespaces: vec![],
            smart_data: None,
        };

        assert_eq!(drive.capacity_gib(), 1024.0);
        assert!(drive.is_healthy());
    }
}
