//! Health Checks
//!
//! Liveness and readiness probes for Kubernetes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Health status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    /// Service is healthy
    Healthy,
    /// Service is degraded but operational
    Degraded,
    /// Service is unhealthy
    Unhealthy,
}

impl HealthStatus {
    /// Check if status is healthy or degraded (operational)
    pub fn is_operational(&self) -> bool {
        matches!(self, HealthStatus::Healthy | HealthStatus::Degraded)
    }

    /// Check if status is healthy
    pub fn is_healthy(&self) -> bool {
        *self == HealthStatus::Healthy
    }
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "Healthy"),
            HealthStatus::Degraded => write!(f, "Degraded"),
            HealthStatus::Unhealthy => write!(f, "Unhealthy"),
        }
    }
}

/// Health check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    /// Check name
    pub name: String,
    /// Status
    pub status: HealthStatus,
    /// Message
    pub message: Option<String>,
    /// Duration of check
    pub duration_ms: u64,
}

impl HealthCheckResult {
    /// Create a healthy result
    pub fn healthy(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Healthy,
            message: None,
            duration_ms: 0,
        }
    }

    /// Create a degraded result
    pub fn degraded(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Degraded,
            message: Some(message.into()),
            duration_ms: 0,
        }
    }

    /// Create an unhealthy result
    pub fn unhealthy(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Unhealthy,
            message: Some(message.into()),
            duration_ms: 0,
        }
    }

    /// Set duration
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration_ms = duration.as_millis() as u64;
        self
    }
}

/// Overall health response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Overall status
    pub status: HealthStatus,
    /// Individual check results
    pub checks: Vec<HealthCheckResult>,
    /// Version
    pub version: String,
    /// Uptime in seconds
    pub uptime_seconds: u64,
}

impl HealthResponse {
    /// Create a new health response
    pub fn new(checks: Vec<HealthCheckResult>, uptime: Duration) -> Self {
        let status = checks
            .iter()
            .map(|c| c.status)
            .max_by_key(|s| match s {
                HealthStatus::Healthy => 0,
                HealthStatus::Degraded => 1,
                HealthStatus::Unhealthy => 2,
            })
            .unwrap_or(HealthStatus::Healthy);

        Self {
            status,
            checks,
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: uptime.as_secs(),
        }
    }
}

/// Health check manager
pub struct HealthCheck {
    /// Start time
    start_time: Instant,
    /// Liveness flag
    live: AtomicBool,
    /// Readiness flag
    ready: AtomicBool,
}

impl HealthCheck {
    /// Create a new health check manager
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            live: AtomicBool::new(true),
            ready: AtomicBool::new(false),
        }
    }

    /// Get uptime
    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Check liveness
    pub fn is_live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }

    /// Check readiness
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    /// Set liveness
    pub fn set_live(&self, live: bool) {
        self.live.store(live, Ordering::Relaxed);
    }

    /// Set readiness
    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::Relaxed);
    }

    /// Get liveness status
    pub fn liveness(&self) -> HealthStatus {
        if self.is_live() {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unhealthy
        }
    }

    /// Get readiness status
    pub fn readiness(&self) -> HealthStatus {
        if self.is_ready() {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unhealthy
        }
    }

    /// Run all health checks
    pub fn check_all(&self) -> HealthResponse {
        let checks = vec![
            HealthCheckResult {
                name: "liveness".to_string(),
                status: self.liveness(),
                message: None,
                duration_ms: 0,
            },
            HealthCheckResult {
                name: "readiness".to_string(),
                status: self.readiness(),
                message: None,
                duration_ms: 0,
            },
        ];

        HealthResponse::new(checks, self.uptime())
    }

    /// Get liveness response
    pub fn liveness_check(&self) -> HealthCheckResult {
        HealthCheckResult {
            name: "liveness".to_string(),
            status: self.liveness(),
            message: None,
            duration_ms: 0,
        }
    }

    /// Get readiness response
    pub fn readiness_check(&self) -> HealthCheckResult {
        HealthCheckResult {
            name: "readiness".to_string(),
            status: self.readiness(),
            message: if !self.is_ready() {
                Some("Service not ready".to_string())
            } else {
                None
            },
            duration_ms: 0,
        }
    }
}

impl Default for HealthCheck {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status() {
        assert!(HealthStatus::Healthy.is_healthy());
        assert!(HealthStatus::Healthy.is_operational());
        assert!(!HealthStatus::Degraded.is_healthy());
        assert!(HealthStatus::Degraded.is_operational());
        assert!(!HealthStatus::Unhealthy.is_healthy());
        assert!(!HealthStatus::Unhealthy.is_operational());
    }

    #[test]
    fn test_health_status_display() {
        assert_eq!(format!("{}", HealthStatus::Healthy), "Healthy");
        assert_eq!(format!("{}", HealthStatus::Degraded), "Degraded");
        assert_eq!(format!("{}", HealthStatus::Unhealthy), "Unhealthy");
    }

    #[test]
    fn test_health_check_result() {
        let healthy = HealthCheckResult::healthy("test");
        assert_eq!(healthy.status, HealthStatus::Healthy);
        assert!(healthy.message.is_none());

        let degraded = HealthCheckResult::degraded("test", "slow");
        assert_eq!(degraded.status, HealthStatus::Degraded);
        assert_eq!(degraded.message, Some("slow".to_string()));

        let unhealthy = HealthCheckResult::unhealthy("test", "failed");
        assert_eq!(unhealthy.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_health_check_manager() {
        let health = HealthCheck::new();

        assert!(health.is_live());
        assert!(!health.is_ready());

        health.set_ready(true);
        assert!(health.is_ready());

        health.set_live(false);
        assert!(!health.is_live());
    }

    #[test]
    fn test_health_response() {
        let checks = vec![
            HealthCheckResult::healthy("check1"),
            HealthCheckResult::degraded("check2", "slow"),
        ];

        let response = HealthResponse::new(checks, Duration::from_secs(60));
        assert_eq!(response.status, HealthStatus::Degraded);
        assert_eq!(response.uptime_seconds, 60);
    }

    #[test]
    fn test_health_check_all() {
        let health = HealthCheck::new();
        health.set_ready(true);

        let response = health.check_all();
        assert_eq!(response.status, HealthStatus::Healthy);
        assert_eq!(response.checks.len(), 2);
    }

    #[test]
    fn test_uptime() {
        let health = HealthCheck::new();
        std::thread::sleep(Duration::from_millis(10));
        assert!(health.uptime() >= Duration::from_millis(10));
    }

    #[test]
    fn test_serialization() {
        let result = HealthCheckResult::healthy("test");
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("Healthy"));

        let deserialized: HealthCheckResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.status, HealthStatus::Healthy);
    }
}
