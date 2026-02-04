//! Metrics Watcher - "The Eyes"
//!
//! Queries Prometheus for volume IOPS metrics to determine
//! which volumes are "hot" (high activity) vs "cold" (low activity).

use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use parking_lot::RwLock;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, instrument, warn};

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the metrics watcher
#[derive(Debug, Clone)]
pub struct MetricsConfig {
    /// Prometheus server URL
    pub prometheus_url: String,

    /// Query timeout
    pub query_timeout: Duration,

    /// Enable caching
    pub cache_enabled: bool,

    /// Cache TTL
    pub cache_ttl: Duration,

    /// Primary metric name to query
    pub metric_name: String,

    /// Fallback metric names
    pub fallback_metrics: Vec<String>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            prometheus_url: "http://prometheus.monitoring.svc.cluster.local:9090".to_string(),
            query_timeout: Duration::from_secs(30),
            cache_enabled: true,
            cache_ttl: Duration::from_secs(30),
            metric_name: "openebs_volume_iops".to_string(),
            fallback_metrics: vec![
                "mayastor_volume_iops".to_string(),
                "mayastor_volume_read_ops".to_string(),
            ],
        }
    }
}

// =============================================================================
// Heat Score
// =============================================================================

/// Heat score for a volume - represents how "hot" (active) a volume is
#[derive(Debug, Clone, Serialize)]
pub struct HeatScore {
    /// Volume ID
    pub volume_id: String,

    /// Time-weighted average IOPS
    pub score: f64,

    /// Read IOPS component
    pub read_iops: f64,

    /// Write IOPS component
    pub write_iops: f64,

    /// Average latency (if available)
    pub latency_us: Option<f64>,

    /// Number of samples used
    pub sample_count: usize,

    /// When this score was calculated
    pub calculated_at: DateTime<Utc>,

    /// Duration over which score was calculated
    pub window: Duration,

    /// Which metric was used
    pub source_metric: String,
}

impl HeatScore {
    /// Create a zero score (for volumes with no metrics)
    pub fn zero(volume_id: &str) -> Self {
        Self {
            volume_id: volume_id.to_string(),
            score: 0.0,
            read_iops: 0.0,
            write_iops: 0.0,
            latency_us: None,
            sample_count: 0,
            calculated_at: Utc::now(),
            window: Duration::ZERO,
            source_metric: "none".to_string(),
        }
    }

    /// Check if this score indicates a "hot" volume
    #[allow(dead_code)]
    pub fn is_hot(&self, threshold: u32) -> bool {
        self.score > threshold as f64
    }

    /// Check if this score indicates a "cold" volume
    #[allow(dead_code)]
    pub fn is_cold(&self, threshold: u32) -> bool {
        self.score < threshold as f64
    }
}

// =============================================================================
// Prometheus Response Types
// =============================================================================

#[derive(Debug, Deserialize)]
struct PrometheusResponse {
    status: String,
    data: PrometheusData,
}

#[derive(Debug, Deserialize)]
struct PrometheusData {
    #[serde(rename = "resultType")]
    #[allow(dead_code)]
    result_type: String,
    result: Vec<PrometheusResult>,
}

#[derive(Debug, Deserialize)]
struct PrometheusResult {
    #[allow(dead_code)]
    metric: serde_json::Value,
    #[serde(default)]
    value: Option<(f64, String)>,
    #[serde(default)]
    values: Option<Vec<(f64, String)>>,
}

// =============================================================================
// Cache Entry
// =============================================================================

#[derive(Debug, Clone)]
struct CacheEntry {
    score: HeatScore,
    expires_at: std::time::Instant,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        std::time::Instant::now() > self.expires_at
    }
}

// =============================================================================
// Metrics Watcher
// =============================================================================

/// Watches and queries volume metrics from Prometheus
pub struct MetricsWatcher {
    config: MetricsConfig,
    client: Client,
    cache: DashMap<String, CacheEntry>,
    healthy: RwLock<bool>,
}

impl MetricsWatcher {
    /// Create a new metrics watcher
    pub fn new(config: MetricsConfig) -> Result<Arc<Self>> {
        let client = Client::builder()
            .timeout(config.query_timeout)
            .build()
            .map_err(|e| Error::Internal(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Arc::new(Self {
            config,
            client,
            cache: DashMap::new(),
            healthy: RwLock::new(true),
        }))
    }

    /// Check if Prometheus is reachable
    #[instrument(skip(self))]
    pub async fn health_check(&self) -> Result<()> {
        let url = format!("{}/-/healthy", self.config.prometheus_url);

        let response = self.client.get(&url).send().await.map_err(|e| {
            *self.healthy.write() = false;
            Error::PrometheusConnection(e)
        })?;

        if response.status().is_success() {
            *self.healthy.write() = true;
            Ok(())
        } else {
            *self.healthy.write() = false;
            Err(Error::PrometheusQuery(format!(
                "Health check failed: {}",
                response.status()
            )))
        }
    }

    /// Check if the watcher is healthy
    #[allow(dead_code)]
    pub fn is_healthy(&self) -> bool {
        *self.healthy.read()
    }

    /// Get heat score for a single volume
    #[instrument(skip(self), fields(volume_id = %volume_id))]
    pub async fn get_heat_score(&self, volume_id: &str, window: Duration) -> Result<HeatScore> {
        // Check cache first
        if self.config.cache_enabled {
            if let Some(entry) = self.cache.get(volume_id) {
                if !entry.is_expired() {
                    debug!("Cache hit for volume {}", volume_id);
                    return Ok(entry.score.clone());
                }
            }
        }

        // Query Prometheus
        let score = self.query_volume_iops(volume_id, window).await?;

        // Update cache
        if self.config.cache_enabled {
            self.cache.insert(
                volume_id.to_string(),
                CacheEntry {
                    score: score.clone(),
                    expires_at: std::time::Instant::now() + self.config.cache_ttl,
                },
            );
        }

        Ok(score)
    }

    /// Get heat scores for multiple volumes efficiently
    #[allow(dead_code)]
    #[instrument(skip(self, volume_ids))]
    pub async fn get_bulk_heat_scores(
        &self,
        volume_ids: &[String],
        window: Duration,
    ) -> Vec<HeatScore> {
        let mut scores = Vec::with_capacity(volume_ids.len());

        // For now, query sequentially (could be parallelized with join_all)
        for volume_id in volume_ids {
            let score = self
                .get_heat_score(volume_id, window)
                .await
                .unwrap_or_else(|e| {
                    warn!("Failed to get heat score for {}: {}", volume_id, e);
                    HeatScore::zero(volume_id)
                });
            scores.push(score);
        }

        scores
    }

    /// Query Prometheus for volume IOPS
    #[instrument(skip(self))]
    async fn query_volume_iops(&self, volume_id: &str, window: Duration) -> Result<HeatScore> {
        // Try primary metric first, then fallbacks
        let metrics_to_try = std::iter::once(self.config.metric_name.clone())
            .chain(self.config.fallback_metrics.iter().cloned());

        // Track the last connection error to propagate if all metrics fail
        let mut last_connection_error: Option<Error> = None;
        let mut any_query_succeeded = false;

        for metric_name in metrics_to_try {
            match self
                .query_metric_for_volume(&metric_name, volume_id, window)
                .await
            {
                Ok(score) if score.sample_count > 0 => {
                    debug!(
                        "Got heat score {} for volume {} using metric {}",
                        score.score, volume_id, metric_name
                    );
                    return Ok(score);
                }
                Ok(_) => {
                    debug!(
                        "No samples found for volume {} using metric {}",
                        volume_id, metric_name
                    );
                    any_query_succeeded = true;
                    continue;
                }
                Err(e) => {
                    debug!(
                        "Failed to query {} for volume {}: {}",
                        metric_name, volume_id, e
                    );
                    // Track connection errors to propagate if all metrics fail
                    if matches!(&e, Error::PrometheusConnection(_)) {
                        last_connection_error = Some(e);
                    }
                    continue;
                }
            }
        }

        // If we had connection errors and no successful queries, propagate the error
        if let Some(conn_err) = last_connection_error {
            if !any_query_succeeded {
                return Err(conn_err);
            }
        }

        // No metrics found but Prometheus was reachable - return zero score
        debug!(
            "No metrics available for volume {}, returning zero score",
            volume_id
        );
        Ok(HeatScore::zero(volume_id))
    }

    /// Query a specific metric for a volume
    async fn query_metric_for_volume(
        &self,
        metric_name: &str,
        volume_id: &str,
        window: Duration,
    ) -> Result<HeatScore> {
        // Build PromQL query for time-weighted average
        // Using rate() to get per-second values over the window
        let window_str = format!("{}s", window.as_secs());
        let query = format!(
            r#"avg_over_time({}{{volume_id="{}"}}[{}])"#,
            metric_name, volume_id, window_str
        );

        let url = format!(
            "{}/api/v1/query?query={}",
            self.config.prometheus_url,
            urlencoding::encode(&query)
        );

        debug!("Querying Prometheus: {}", query);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(Error::PrometheusConnection)?;

        if !response.status().is_success() {
            return Err(Error::PrometheusQuery(format!(
                "Query failed with status: {}",
                response.status()
            )));
        }

        let prom_response: PrometheusResponse = response
            .json()
            .await
            .map_err(|e| Error::PrometheusResponseParse(e.to_string()))?;

        if prom_response.status != "success" {
            return Err(Error::PrometheusQuery(format!(
                "Prometheus returned status: {}",
                prom_response.status
            )));
        }

        // Parse the result
        let score = if let Some(result) = prom_response.data.result.first() {
            if let Some((_, value_str)) = &result.value {
                let value: f64 = value_str
                    .parse()
                    .map_err(|_| Error::PrometheusResponseParse("Invalid float value".into()))?;

                // Filter out NaN and Inf
                let value = if value.is_finite() { value } else { 0.0 };

                HeatScore {
                    volume_id: volume_id.to_string(),
                    score: value,
                    read_iops: value / 2.0, // Approximate split
                    write_iops: value / 2.0,
                    latency_us: None,
                    sample_count: 1,
                    calculated_at: Utc::now(),
                    window,
                    source_metric: metric_name.to_string(),
                }
            } else {
                HeatScore::zero(volume_id)
            }
        } else {
            HeatScore::zero(volume_id)
        };

        Ok(score)
    }

    /// Query for range data (for more accurate averaging)
    #[allow(dead_code)]
    async fn query_range(
        &self,
        metric_name: &str,
        volume_id: &str,
        window: Duration,
    ) -> Result<HeatScore> {
        let end = Utc::now();
        let start = end - chrono::Duration::from_std(window).unwrap();

        let query = format!(r#"{}{{volume_id="{}"}}"#, metric_name, volume_id);

        let url = format!(
            "{}/api/v1/query_range?query={}&start={}&end={}&step=60s",
            self.config.prometheus_url,
            urlencoding::encode(&query),
            start.timestamp(),
            end.timestamp()
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(Error::PrometheusConnection)?;

        let prom_response: PrometheusResponse = response
            .json()
            .await
            .map_err(|e| Error::PrometheusResponseParse(e.to_string()))?;

        // Calculate time-weighted average with exponential decay
        // More recent samples have higher weight
        if let Some(result) = prom_response.data.result.first() {
            if let Some(values) = &result.values {
                let now = Utc::now().timestamp() as f64;
                let decay_constant = 0.001; // Adjust for decay rate

                let mut weighted_sum = 0.0;
                let mut weight_sum = 0.0;

                for (timestamp, value_str) in values {
                    if let Ok(value) = value_str.parse::<f64>() {
                        if value.is_finite() {
                            let age = now - timestamp;
                            let weight = (-decay_constant * age).exp();
                            weighted_sum += value * weight;
                            weight_sum += weight;
                        }
                    }
                }

                let score = if weight_sum > 0.0 {
                    weighted_sum / weight_sum
                } else {
                    0.0
                };

                return Ok(HeatScore {
                    volume_id: volume_id.to_string(),
                    score,
                    read_iops: score / 2.0,
                    write_iops: score / 2.0,
                    latency_us: None,
                    sample_count: values.len(),
                    calculated_at: Utc::now(),
                    window,
                    source_metric: metric_name.to_string(),
                });
            }
        }

        Ok(HeatScore::zero(volume_id))
    }

    /// Invalidate cache entry for a volume
    #[allow(dead_code)]
    pub fn invalidate_cache(&self, volume_id: &str) {
        self.cache.remove(volume_id);
    }

    /// Clear entire cache
    #[allow(dead_code)]
    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// Get cache statistics
    #[allow(dead_code)]
    pub fn cache_stats(&self) -> CacheStats {
        let total = self.cache.len();
        let expired = self.cache.iter().filter(|entry| entry.is_expired()).count();

        CacheStats {
            total_entries: total,
            expired_entries: expired,
            active_entries: total - expired,
        }
    }
}

/// Cache statistics
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
pub struct CacheStats {
    pub total_entries: usize,
    pub expired_entries: usize,
    pub active_entries: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // MetricsConfig Tests
    // =========================================================================

    #[test]
    fn test_metrics_config_default() {
        let config = MetricsConfig::default();

        assert_eq!(
            config.prometheus_url,
            "http://prometheus.monitoring.svc.cluster.local:9090"
        );
        assert_eq!(config.query_timeout, Duration::from_secs(30));
        assert!(config.cache_enabled);
        assert_eq!(config.cache_ttl, Duration::from_secs(30));
        assert_eq!(config.metric_name, "openebs_volume_iops");
        assert_eq!(config.fallback_metrics.len(), 2);
        assert!(config
            .fallback_metrics
            .contains(&"mayastor_volume_iops".to_string()));
        assert!(config
            .fallback_metrics
            .contains(&"mayastor_volume_read_ops".to_string()));
    }

    #[test]
    fn test_metrics_config_custom() {
        let config = MetricsConfig {
            prometheus_url: "http://localhost:9090".to_string(),
            query_timeout: Duration::from_secs(60),
            cache_enabled: false,
            cache_ttl: Duration::from_secs(120),
            metric_name: "custom_metric".to_string(),
            fallback_metrics: vec!["fallback1".to_string()],
        };

        assert_eq!(config.prometheus_url, "http://localhost:9090");
        assert_eq!(config.query_timeout, Duration::from_secs(60));
        assert!(!config.cache_enabled);
        assert_eq!(config.cache_ttl, Duration::from_secs(120));
        assert_eq!(config.metric_name, "custom_metric");
        assert_eq!(config.fallback_metrics, vec!["fallback1".to_string()]);
    }

    #[test]
    fn test_metrics_config_clone() {
        let config = MetricsConfig::default();
        let cloned = config.clone();

        assert_eq!(config.prometheus_url, cloned.prometheus_url);
        assert_eq!(config.metric_name, cloned.metric_name);
    }

    // =========================================================================
    // HeatScore Tests
    // =========================================================================

    #[test]
    fn test_heat_score_zero() {
        let score = HeatScore::zero("vol-123");

        assert_eq!(score.volume_id, "vol-123");
        assert_eq!(score.score, 0.0);
        assert_eq!(score.read_iops, 0.0);
        assert_eq!(score.write_iops, 0.0);
        assert!(score.latency_us.is_none());
        assert_eq!(score.sample_count, 0);
        assert_eq!(score.window, Duration::ZERO);
        assert_eq!(score.source_metric, "none");
    }

    #[test]
    fn test_heat_score_thresholds() {
        let score = HeatScore {
            volume_id: "test".into(),
            score: 3000.0,
            read_iops: 1500.0,
            write_iops: 1500.0,
            latency_us: None,
            sample_count: 10,
            calculated_at: Utc::now(),
            window: Duration::from_secs(3600),
            source_metric: "test".into(),
        };

        assert!(score.is_hot(2000));
        assert!(!score.is_hot(5000));
        assert!(!score.is_cold(500));
        assert!(score.is_cold(5000));
    }

    #[test]
    fn test_heat_score_boundary_conditions() {
        // Test exact threshold boundary
        let score = HeatScore {
            volume_id: "test".into(),
            score: 1000.0,
            read_iops: 500.0,
            write_iops: 500.0,
            latency_us: None,
            sample_count: 1,
            calculated_at: Utc::now(),
            window: Duration::from_secs(3600),
            source_metric: "test".into(),
        };

        // At exactly 1000, is_hot(1000) should be false (not strictly greater)
        assert!(!score.is_hot(1000));
        // At exactly 1000, is_cold(1000) should be false (not strictly less)
        assert!(!score.is_cold(1000));

        // Just above/below threshold
        assert!(score.is_hot(999));
        assert!(score.is_cold(1001));
    }

    #[test]
    fn test_heat_score_zero_is_cold() {
        let score = HeatScore::zero("vol-123");

        // Zero score should always be cold for any positive threshold
        assert!(score.is_cold(1));
        assert!(score.is_cold(100));
        assert!(score.is_cold(10000));

        // Zero score is never hot
        assert!(!score.is_hot(0)); // 0.0 > 0 is false
        assert!(!score.is_hot(1));
    }

    #[test]
    fn test_heat_score_high_values() {
        let score = HeatScore {
            volume_id: "hot-volume".into(),
            score: 100000.0,
            read_iops: 60000.0,
            write_iops: 40000.0,
            latency_us: Some(150.0),
            sample_count: 100,
            calculated_at: Utc::now(),
            window: Duration::from_secs(7200),
            source_metric: "openebs_volume_iops".into(),
        };

        assert!(score.is_hot(5000));
        assert!(score.is_hot(50000));
        assert!(score.is_hot(99999));
        assert!(!score.is_cold(1000));
    }

    #[test]
    fn test_heat_score_with_latency() {
        let score = HeatScore {
            volume_id: "test".into(),
            score: 5000.0,
            read_iops: 3000.0,
            write_iops: 2000.0,
            latency_us: Some(250.5),
            sample_count: 50,
            calculated_at: Utc::now(),
            window: Duration::from_secs(3600),
            source_metric: "test".into(),
        };

        assert_eq!(score.latency_us, Some(250.5));
        assert_eq!(score.read_iops + score.write_iops, 5000.0);
    }

    #[test]
    fn test_heat_score_clone() {
        let score = HeatScore {
            volume_id: "test".into(),
            score: 1234.5,
            read_iops: 600.0,
            write_iops: 634.5,
            latency_us: Some(100.0),
            sample_count: 10,
            calculated_at: Utc::now(),
            window: Duration::from_secs(3600),
            source_metric: "test".into(),
        };

        let cloned = score.clone();

        assert_eq!(score.volume_id, cloned.volume_id);
        assert_eq!(score.score, cloned.score);
        assert_eq!(score.read_iops, cloned.read_iops);
        assert_eq!(score.write_iops, cloned.write_iops);
        assert_eq!(score.latency_us, cloned.latency_us);
        assert_eq!(score.sample_count, cloned.sample_count);
    }

    #[test]
    fn test_heat_score_serializes() {
        let score = HeatScore {
            volume_id: "vol-test".into(),
            score: 2500.0,
            read_iops: 1500.0,
            write_iops: 1000.0,
            latency_us: Some(200.0),
            sample_count: 5,
            calculated_at: Utc::now(),
            window: Duration::from_secs(3600),
            source_metric: "openebs_volume_iops".into(),
        };

        let json = serde_json::to_string(&score).unwrap();

        assert!(json.contains("\"volume_id\":\"vol-test\""));
        assert!(json.contains("\"score\":2500.0"));
        assert!(json.contains("\"read_iops\":1500.0"));
        assert!(json.contains("\"write_iops\":1000.0"));
        assert!(json.contains("\"latency_us\":200.0"));
        assert!(json.contains("\"sample_count\":5"));
        assert!(json.contains("\"source_metric\":\"openebs_volume_iops\""));
    }

    #[test]
    fn test_heat_score_zero_serializes() {
        let score = HeatScore::zero("vol-empty");
        let json = serde_json::to_string(&score).unwrap();

        assert!(json.contains("\"volume_id\":\"vol-empty\""));
        assert!(json.contains("\"score\":0.0"));
        assert!(json.contains("\"sample_count\":0"));
        assert!(json.contains("\"source_metric\":\"none\""));
    }

    // =========================================================================
    // CacheStats Tests
    // =========================================================================

    #[test]
    fn test_cache_stats_creation() {
        let stats = CacheStats {
            total_entries: 100,
            expired_entries: 20,
            active_entries: 80,
        };

        assert_eq!(stats.total_entries, 100);
        assert_eq!(stats.expired_entries, 20);
        assert_eq!(stats.active_entries, 80);
    }

    #[test]
    fn test_cache_stats_clone() {
        let stats = CacheStats {
            total_entries: 50,
            expired_entries: 10,
            active_entries: 40,
        };

        let cloned = stats.clone();

        assert_eq!(stats.total_entries, cloned.total_entries);
        assert_eq!(stats.expired_entries, cloned.expired_entries);
        assert_eq!(stats.active_entries, cloned.active_entries);
    }

    #[test]
    fn test_cache_stats_serializes() {
        let stats = CacheStats {
            total_entries: 100,
            expired_entries: 25,
            active_entries: 75,
        };

        let json = serde_json::to_string(&stats).unwrap();

        assert!(json.contains("\"total_entries\":100"));
        assert!(json.contains("\"expired_entries\":25"));
        assert!(json.contains("\"active_entries\":75"));
    }

    #[test]
    fn test_cache_stats_empty() {
        let stats = CacheStats {
            total_entries: 0,
            expired_entries: 0,
            active_entries: 0,
        };

        assert_eq!(stats.total_entries, 0);
        assert_eq!(stats.expired_entries, 0);
        assert_eq!(stats.active_entries, 0);
    }

    // =========================================================================
    // CacheEntry Tests
    // =========================================================================

    #[test]
    fn test_cache_entry_not_expired() {
        let entry = CacheEntry {
            score: HeatScore::zero("test"),
            expires_at: std::time::Instant::now() + Duration::from_secs(60),
        };

        assert!(!entry.is_expired());
    }

    #[test]
    fn test_cache_entry_expired() {
        let entry = CacheEntry {
            score: HeatScore::zero("test"),
            expires_at: std::time::Instant::now() - Duration::from_secs(1),
        };

        assert!(entry.is_expired());
    }

    #[test]
    fn test_cache_entry_clone() {
        let entry = CacheEntry {
            score: HeatScore::zero("test"),
            expires_at: std::time::Instant::now() + Duration::from_secs(60),
        };

        let cloned = entry.clone();

        assert_eq!(entry.score.volume_id, cloned.score.volume_id);
        assert_eq!(entry.expires_at, cloned.expires_at);
    }

    // =========================================================================
    // Prometheus Response Parsing Tests
    // =========================================================================

    #[test]
    fn test_prometheus_response_deserialize() {
        let json = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": [
                    {
                        "metric": {"volume_id": "vol-123"},
                        "value": [1234567890.123, "5000"]
                    }
                ]
            }
        }"#;

        let response: PrometheusResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.status, "success");
        assert_eq!(response.data.result_type, "vector");
        assert_eq!(response.data.result.len(), 1);

        let result = &response.data.result[0];
        assert!(result.value.is_some());

        let (timestamp, value) = result.value.as_ref().unwrap();
        assert!(*timestamp > 0.0);
        assert_eq!(value, "5000");
    }

    #[test]
    fn test_prometheus_response_empty_result() {
        let json = r#"{
            "status": "success",
            "data": {
                "resultType": "vector",
                "result": []
            }
        }"#;

        let response: PrometheusResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.status, "success");
        assert!(response.data.result.is_empty());
    }

    #[test]
    fn test_prometheus_response_range_values() {
        let json = r#"{
            "status": "success",
            "data": {
                "resultType": "matrix",
                "result": [
                    {
                        "metric": {"volume_id": "vol-123"},
                        "values": [
                            [1234567890.0, "1000"],
                            [1234567950.0, "1500"],
                            [1234568010.0, "2000"]
                        ]
                    }
                ]
            }
        }"#;

        let response: PrometheusResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.status, "success");
        assert_eq!(response.data.result_type, "matrix");

        let result = &response.data.result[0];
        assert!(result.values.is_some());

        let values = result.values.as_ref().unwrap();
        assert_eq!(values.len(), 3);
        assert_eq!(values[0].1, "1000");
        assert_eq!(values[1].1, "1500");
        assert_eq!(values[2].1, "2000");
    }

    #[test]
    fn test_prometheus_response_error_status() {
        let json = r#"{
            "status": "error",
            "data": {
                "resultType": "vector",
                "result": []
            }
        }"#;

        let response: PrometheusResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.status, "error");
    }

    // =========================================================================
    // MetricsWatcher Creation Tests
    // =========================================================================

    #[test]
    fn test_metrics_watcher_new() {
        let config = MetricsConfig::default();
        let watcher = MetricsWatcher::new(config);

        assert!(watcher.is_ok());
    }

    #[test]
    fn test_metrics_watcher_with_custom_config() {
        let config = MetricsConfig {
            prometheus_url: "http://custom:9090".to_string(),
            query_timeout: Duration::from_secs(10),
            cache_enabled: false,
            cache_ttl: Duration::from_secs(60),
            metric_name: "custom".to_string(),
            fallback_metrics: vec![],
        };

        let watcher = MetricsWatcher::new(config);
        assert!(watcher.is_ok());
    }

    #[test]
    fn test_metrics_watcher_initial_health_state() {
        let config = MetricsConfig::default();
        let watcher = MetricsWatcher::new(config).unwrap();

        // Initially healthy (optimistic)
        assert!(watcher.is_healthy());
    }

    #[test]
    fn test_metrics_watcher_cache_operations() {
        let config = MetricsConfig::default();
        let watcher = MetricsWatcher::new(config).unwrap();

        // Initially empty cache
        let stats = watcher.cache_stats();
        assert_eq!(stats.total_entries, 0);

        // Clear empty cache (should not panic)
        watcher.clear_cache();

        // Invalidate non-existent entry (should not panic)
        watcher.invalidate_cache("non-existent");
    }
}
