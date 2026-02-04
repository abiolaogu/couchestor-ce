//! Prometheus Metrics Adapter
//!
//! Implements the `MetricsProvider` port using Prometheus as the metrics source.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;

use crate::domain::ports::{HeatScore, MetricsProvider, VolumeId};
use crate::error::Result;
use crate::metrics::MetricsWatcher;

/// Default sampling window for IOPS metrics.
#[allow(dead_code)]
const DEFAULT_SAMPLING_WINDOW: Duration = Duration::from_secs(300); // 5 minutes

/// Prometheus-based metrics provider adapter.
///
/// Wraps the existing `MetricsWatcher` to implement the `MetricsProvider` port.
#[allow(dead_code)]
pub struct PrometheusMetricsAdapter {
    watcher: Arc<MetricsWatcher>,
    sampling_window: Duration,
}

#[allow(dead_code)]
impl PrometheusMetricsAdapter {
    /// Create a new Prometheus metrics adapter.
    pub fn new(watcher: Arc<MetricsWatcher>) -> Self {
        Self {
            watcher,
            sampling_window: DEFAULT_SAMPLING_WINDOW,
        }
    }

    /// Create with a custom sampling window.
    pub fn with_sampling_window(watcher: Arc<MetricsWatcher>, sampling_window: Duration) -> Self {
        Self {
            watcher,
            sampling_window,
        }
    }

    /// Create from an existing MetricsWatcher (takes ownership).
    pub fn from_watcher(watcher: MetricsWatcher) -> Self {
        Self {
            watcher: Arc::new(watcher),
            sampling_window: DEFAULT_SAMPLING_WINDOW,
        }
    }

    /// Get a reference to the underlying watcher.
    pub fn watcher(&self) -> &MetricsWatcher {
        &self.watcher
    }
}

impl std::fmt::Debug for PrometheusMetricsAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrometheusMetricsAdapter")
            .field("sampling_window", &self.sampling_window)
            .finish()
    }
}

#[async_trait]
impl MetricsProvider for PrometheusMetricsAdapter {
    async fn get_volume_iops(&self, volume_id: &VolumeId) -> Result<f64> {
        let heat_score = self
            .watcher
            .get_heat_score(&volume_id.0, self.sampling_window)
            .await?;
        Ok(heat_score.score)
    }

    async fn get_heat_score(&self, volume_id: &VolumeId) -> Result<HeatScore> {
        let watcher_score = self
            .watcher
            .get_heat_score(&volume_id.0, self.sampling_window)
            .await?;

        Ok(HeatScore {
            iops: watcher_score.score,
            weighted_avg: watcher_score.score,
            timestamp: Utc::now(),
        })
    }

    async fn get_heat_scores(&self, volume_ids: &[VolumeId]) -> Result<Vec<(VolumeId, HeatScore)>> {
        let mut results = Vec::with_capacity(volume_ids.len());

        for volume_id in volume_ids {
            let heat_score = self.get_heat_score(volume_id).await?;
            results.push((volume_id.clone(), heat_score));
        }

        Ok(results)
    }

    async fn health_check(&self) -> Result<bool> {
        self.watcher.health_check().await.map(|_| true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::MetricsConfig;

    fn test_config() -> MetricsConfig {
        MetricsConfig {
            prometheus_url: "http://localhost:9090".to_string(),
            query_timeout: Duration::from_secs(5),
            cache_enabled: true,
            cache_ttl: Duration::from_secs(30),
            metric_name: "test_metric".to_string(),
            fallback_metrics: vec![],
        }
    }

    #[test]
    fn test_adapter_creation() {
        let config = test_config();
        let watcher = MetricsWatcher::new(config).unwrap();
        // MetricsWatcher::new returns Arc<MetricsWatcher>, so use new() not from_watcher()
        let adapter = PrometheusMetricsAdapter::new(watcher);

        // Verify adapter was created successfully
        assert!(format!("{:?}", adapter).contains("PrometheusMetricsAdapter"));
    }

    #[test]
    fn test_adapter_with_custom_window() {
        let config = test_config();
        // MetricsWatcher::new already returns Arc<MetricsWatcher>
        let watcher = MetricsWatcher::new(config).unwrap();
        let custom_window = Duration::from_secs(600);
        let adapter = PrometheusMetricsAdapter::with_sampling_window(watcher, custom_window);

        assert_eq!(adapter.sampling_window, custom_window);
    }

    // =========================================================================
    // Prometheus Unavailability Tests (MET-006)
    // =========================================================================

    #[tokio::test]
    async fn test_prometheus_connection_refused() {
        // Test behavior when Prometheus is completely down (connection refused)
        let config = MetricsConfig {
            prometheus_url: "http://localhost:19999".to_string(), // Non-existent port
            query_timeout: Duration::from_secs(1), // Short timeout for faster test
            cache_enabled: false,
            cache_ttl: Duration::from_secs(30),
            metric_name: "test_metric".to_string(),
            fallback_metrics: vec![],
        };

        let watcher = MetricsWatcher::new(config).unwrap();
        let adapter = PrometheusMetricsAdapter::new(watcher);
        let volume_id = VolumeId::new("vol-connection-refused");

        // Should return error (not panic)
        let result = adapter.get_volume_iops(&volume_id).await;
        assert!(result.is_err());

        // Error should be PrometheusConnection variant
        match result {
            Err(crate::error::Error::PrometheusConnection(_)) => {
                // Expected error type
            }
            other => panic!("Expected PrometheusConnection error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_prometheus_timeout() {
        // Test behavior when Prometheus is slow/unresponsive
        let config = MetricsConfig {
            prometheus_url: "http://192.0.2.1:9090".to_string(), // Non-routable IP (RFC 5737)
            query_timeout: Duration::from_millis(100), // Very short timeout
            cache_enabled: false,
            cache_ttl: Duration::from_secs(30),
            metric_name: "test_metric".to_string(),
            fallback_metrics: vec![],
        };

        let watcher = MetricsWatcher::new(config).unwrap();
        let adapter = PrometheusMetricsAdapter::new(watcher);
        let volume_id = VolumeId::new("vol-timeout");

        // Should timeout and return error
        let result = adapter.get_volume_iops(&volume_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_prometheus_returns_zero_on_no_data() {
        // Test that missing metrics return zero score (not error)
        // This tests the existing behavior in query_volume_iops (line 313-318)
        let config = MetricsConfig {
            prometheus_url: "http://localhost:19999".to_string(),
            query_timeout: Duration::from_secs(1),
            cache_enabled: false,
            cache_ttl: Duration::from_secs(30),
            metric_name: "nonexistent_metric".to_string(),
            fallback_metrics: vec!["also_nonexistent".to_string()],
        };

        let watcher = MetricsWatcher::new(config).unwrap();
        let adapter = PrometheusMetricsAdapter::new(watcher);
        let volume_id = VolumeId::new("vol-no-data");

        // When Prometheus is down, this will error (not return zero)
        // The zero-score behavior only happens when Prometheus is UP but has no data
        let result = adapter.get_volume_iops(&volume_id).await;

        // Should error because Prometheus is unreachable
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_heat_scores_partial_failure() {
        // Test bulk query behavior when some volumes fail
        let config = MetricsConfig {
            prometheus_url: "http://localhost:19999".to_string(),
            query_timeout: Duration::from_secs(1),
            cache_enabled: false,
            cache_ttl: Duration::from_secs(30),
            metric_name: "test_metric".to_string(),
            fallback_metrics: vec![],
        };

        let watcher = MetricsWatcher::new(config).unwrap();
        let adapter = PrometheusMetricsAdapter::new(watcher);

        let volume_ids = vec![
            VolumeId::new("vol-1"),
            VolumeId::new("vol-2"),
            VolumeId::new("vol-3"),
        ];

        // Should return error for all volumes when Prometheus is down
        let result = adapter.get_heat_scores(&volume_ids).await;

        // get_heat_scores() fails fast on first error (line 95)
        // This is expected behavior - if Prometheus is down, don't continue querying
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_health_check_failure() {
        // Test health check when Prometheus is unavailable
        let config = MetricsConfig {
            prometheus_url: "http://localhost:19999".to_string(),
            query_timeout: Duration::from_secs(1),
            cache_enabled: false,
            cache_ttl: Duration::from_secs(30),
            metric_name: "test_metric".to_string(),
            fallback_metrics: vec![],
        };

        let watcher = MetricsWatcher::new(config).unwrap();
        let adapter = PrometheusMetricsAdapter::new(watcher);

        // Health check should fail gracefully
        let result = adapter.health_check().await;
        assert!(result.is_err());

        // Watcher should track unhealthy state
        assert!(!adapter.watcher().is_healthy());
    }

    #[test]
    fn test_adapter_watcher_access() {
        // Test that we can access the underlying watcher for health checks
        let config = test_config();
        let watcher = MetricsWatcher::new(config).unwrap();
        let adapter = PrometheusMetricsAdapter::new(watcher.clone());

        // Should be able to access watcher
        let watcher_ref = adapter.watcher();

        // Initial state should be healthy (optimistic)
        assert!(watcher_ref.is_healthy());
    }
}
