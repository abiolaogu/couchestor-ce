//! Error types for the Smart Storage Operator

use thiserror::Error;

/// Result type alias using our Error type
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur in the Smart Storage Operator
#[derive(Error, Debug)]
#[allow(clippy::enum_variant_names)]
#[allow(dead_code)]
pub enum Error {
    /// Kubernetes API error
    #[error("Kubernetes API error: {0}")]
    Kube(#[from] kube::Error),

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Prometheus connection error
    #[error("Prometheus connection error: {0}")]
    PrometheusConnection(#[source] reqwest::Error),

    /// Prometheus query error
    #[error("Prometheus query error: {0}")]
    PrometheusQuery(String),

    /// Prometheus response parse error
    #[error("Failed to parse Prometheus response: {0}")]
    PrometheusResponseParse(String),

    /// Migration already in progress
    #[error("Migration already in progress for volume: {volume_name}")]
    MigrationInProgress { volume_name: String },

    /// Migration failed
    #[error("Migration failed for volume {volume_name}: {reason}")]
    MigrationFailed { volume_name: String, reason: String },

    /// Migration timeout
    #[error("Migration timed out for volume {volume_name} after {duration}")]
    MigrationTimeout {
        volume_name: String,
        duration: String,
    },

    /// Replica sync failed
    #[error("Replica sync failed: {0}")]
    ReplicaSyncFailed(String),

    /// No suitable pool found
    #[error("No suitable pool found for tier: {tier}")]
    NoSuitablePool { tier: String },

    /// Duration parse error
    #[error("Failed to parse duration: {0}")]
    DurationParse(String),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),

    // =========================================================================
    // Erasure Coding Errors
    // =========================================================================
    /// EC encoding failed
    #[error("EC encoding failed: {0}")]
    EcEncodingFailed(String),

    /// EC reconstruction failed
    #[error("EC reconstruction failed for stripe {stripe_id}: {reason}")]
    EcReconstructionFailed { stripe_id: u64, reason: String },

    /// Insufficient shards for reconstruction
    #[error("Insufficient shards for reconstruction: have {available}, need {required}")]
    InsufficientShards { available: usize, required: usize },

    /// EC policy not found
    #[error("EC policy not found: {0}")]
    EcPolicyNotFound(String),

    /// EC stripe not found
    #[error("EC stripe not found: {0}")]
    EcStripeNotFound(String),

    /// Invalid EC configuration
    #[error("Invalid EC configuration: {0}")]
    InvalidEcConfig(String),

    /// EC destage failed
    #[error("EC destage failed for volume {volume_id}: {reason}")]
    EcDestageFailed { volume_id: String, reason: String },

    // =========================================================================
    // SPDK / DMA Errors
    // =========================================================================
    /// DMA buffer allocation failed
    #[error("DMA allocation failed for size {size}: {reason}")]
    DmaAllocationFailed { size: usize, reason: String },

    /// SPDK initialization failed
    #[error("SPDK initialization failed: {0}")]
    SpdkInitFailed(String),

    /// SPDK bdev operation failed
    #[error("SPDK bdev operation failed: {0}")]
    SpdkBdevError(String),

    /// ISA-L encoding error
    #[error("ISA-L encoding error: {0}")]
    IsalEncodingError(String),

    /// ISA-L matrix operation failed
    #[error("ISA-L matrix operation failed: {0}")]
    IsalMatrixError(String),

    // =========================================================================
    // RustFS Errors
    // =========================================================================
    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// Tenant not found
    #[error("Tenant not found: {0}")]
    TenantNotFound(String),

    /// Quota exceeded
    #[error("Quota exceeded: {0}")]
    QuotaExceeded(String),

    /// Rate limit exceeded
    #[error("Rate limit exceeded: {0}")]
    RateLimitExceeded(String),

    /// Replication error
    #[error("Replication error: {0}")]
    ReplicationError(String),

    // =========================================================================
    // Hardware Discovery Errors
    // =========================================================================
    /// Hardware discovery error
    #[error("Hardware discovery error: {0}")]
    HardwareDiscovery(String),

    /// NVMe command failed
    #[error("NVMe command '{command}' failed: {reason}")]
    NvmeCommand { command: String, reason: String },

    /// SMART data unavailable
    #[error("SMART data unavailable for device: {device}")]
    SmartUnavailable { device: String },

    // =========================================================================
    // Compression Errors
    // =========================================================================
    /// Compression failed
    #[error("Compression with {algorithm} failed: {reason}")]
    CompressionFailed { algorithm: String, reason: String },

    /// Decompression failed
    #[error("Decompression with {algorithm} failed: {reason}")]
    DecompressionFailed { algorithm: String, reason: String },
}
