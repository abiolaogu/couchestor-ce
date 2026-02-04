//! RustFS Community Edition Integration Tests
//!
//! Tests for CE features:
//! - Feature 1: Multi-Tier Caching System
//! - Feature 2: Observability and Monitoring
//!
//! Enterprise features (replication, tenancy) are available in CoucheStor EE.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;

// =============================================================================
// Feature 1: Multi-Tier Caching System Tests
// =============================================================================

mod cache_tests {
    use super::*;
    use couchestor::rustfs::cache::{
        CacheConfig, CacheEntry, CacheKey, CacheManager, CacheTier, InMemoryL3Backend,
    };

    #[tokio::test]
    async fn test_three_tier_structure() {
        let manager = CacheManager::in_memory();

        assert!(manager.l1().len() == 0);
        assert!(manager.l2().len() == 0);

        let key = CacheKey::new("bucket", "test");
        let result = manager.get(&key).await;
        assert!(result.is_none());

        assert_eq!(format!("{}", CacheTier::L1), "L1 (RAM)");
        assert_eq!(format!("{}", CacheTier::L2), "L2 (NVMe)");
        assert_eq!(format!("{}", CacheTier::L3), "L3 (Cold)");
    }

    #[tokio::test]
    async fn test_size_based_tier_placement() {
        let mut config = CacheConfig::default();
        config.promotion_policy.l1_max_size = 1024;

        let backend = Arc::new(InMemoryL3Backend::new());
        let manager = CacheManager::with_config(config, backend);

        let small_key = CacheKey::new("bucket", "small");
        let small_entry = CacheEntry::new(Bytes::from(vec![0u8; 100]));
        let tier = manager.put(small_key.clone(), small_entry).await.unwrap();
        assert_eq!(tier, CacheTier::L1);

        let large_key = CacheKey::new("bucket", "large");
        let large_entry = CacheEntry::new(Bytes::from(vec![0u8; 2048]));
        let tier = manager.put(large_key.clone(), large_entry).await.unwrap();
        assert_eq!(tier, CacheTier::L2);
    }

    #[tokio::test]
    async fn test_cache_lookup_flow() {
        let manager = CacheManager::in_memory();

        let key = CacheKey::new("bucket", "object");
        let entry = CacheEntry::new(Bytes::from("test data"));

        manager.put(key.clone(), entry).await.unwrap();

        let result = manager.get(&key).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().tier, CacheTier::L1);

        let missing_key = CacheKey::new("bucket", "nonexistent");
        let result = manager.get(&missing_key).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_l3_lookup_with_promotion() {
        let mut config = CacheConfig::default();
        config.auto_promotion = true;
        config.promotion_policy.l1_promotion_threshold = 1;

        let backend = Arc::new(InMemoryL3Backend::new());
        let manager = CacheManager::with_config(config, backend);

        let key = CacheKey::new("bucket", "cold-object");
        let entry = CacheEntry::new(Bytes::from("cold data"));

        manager.l3().put(&key, &entry).await.unwrap();

        let result = manager.get(&key).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().tier, CacheTier::L3);

        assert!(manager.l1().contains(&key));
    }

    #[tokio::test]
    async fn test_statistics_tracking() {
        let manager = CacheManager::in_memory();

        let key = CacheKey::new("bucket", "object");
        let entry = CacheEntry::new(Bytes::from("data"));
        manager.put(key.clone(), entry).await.unwrap();

        manager.get(&key).await;
        manager.get(&key).await;

        manager.get(&CacheKey::new("bucket", "miss1")).await;
        manager.get(&CacheKey::new("bucket", "miss2")).await;

        let metrics = manager.metrics();

        assert_eq!(metrics.l1_hits, 2);
        assert!(metrics.l3_misses >= 2);
        assert!(metrics.overall_hit_ratio >= 0.0);
    }

    #[tokio::test]
    async fn test_delete_from_all_tiers() {
        let manager = CacheManager::in_memory();

        let key = CacheKey::new("bucket", "to-delete");
        let entry = CacheEntry::new(Bytes::from("delete me"));

        manager.put(key.clone(), entry).await.unwrap();
        assert!(manager.exists(&key).await.unwrap());

        let deleted = manager.delete(&key).await.unwrap();
        assert!(deleted);
        assert!(!manager.exists(&key).await.unwrap());
    }

    #[tokio::test]
    async fn test_clear_all_caches() {
        let manager = CacheManager::in_memory();

        for i in 0..10 {
            let key = CacheKey::new("bucket", format!("object-{}", i));
            let entry = CacheEntry::new(Bytes::from(format!("data-{}", i)));
            manager.put(key, entry).await.unwrap();
        }

        assert_eq!(manager.total_cached_entries(), 10);

        manager.clear().await;
        assert_eq!(manager.total_cached_entries(), 0);
    }

    #[tokio::test]
    async fn test_entry_integrity() {
        let data = Bytes::from("integrity check data");
        let entry = CacheEntry::new(data);
        assert!(entry.verify_integrity());
    }

    #[tokio::test]
    async fn test_entry_ttl() {
        let entry = CacheEntry::with_ttl(Bytes::from("data"), Duration::from_secs(3600));
        assert!(!entry.is_expired());

        let no_ttl = CacheEntry::new(Bytes::from("data"));
        assert!(!no_ttl.is_expired());
    }

    #[test]
    fn test_cache_line_alignment() {
        use couchestor::rustfs::cache::CACHE_LINE_SIZE;
        assert_eq!(CACHE_LINE_SIZE, 64);
        assert_eq!(std::mem::align_of::<CacheKey>(), 64);
    }

    #[test]
    fn test_sharding_distribution() {
        use couchestor::rustfs::cache::SHARD_COUNT;
        assert_eq!(SHARD_COUNT, 1024);
        assert!(SHARD_COUNT.is_power_of_two());

        let mut shard_counts = vec![0usize; SHARD_COUNT];
        for i in 0..10000 {
            let key = CacheKey::new("bucket", format!("key-{}", i));
            let idx = key.shard_index(SHARD_COUNT);
            assert!(idx < SHARD_COUNT);
            shard_counts[idx] += 1;
        }

        let max_count = *shard_counts.iter().max().unwrap();
        assert!(max_count < 500, "Uneven shard distribution: max = {}", max_count);
    }
}

// =============================================================================
// Feature 2: Observability and Monitoring Tests
// =============================================================================

mod monitoring_tests {
    use super::*;
    use couchestor::rustfs::monitoring::{
        Counter, Gauge, HealthCheck, HealthCheckResult, HealthResponse, HealthStatus,
        Histogram, MetricsCollector, ObservabilityConfig,
    };

    #[test]
    fn test_counter_metrics() {
        let counter = Counter::new();
        assert_eq!(counter.get(), 0);

        counter.inc();
        counter.inc();
        assert_eq!(counter.get(), 2);

        counter.add(10);
        assert_eq!(counter.get(), 12);

        counter.reset();
        assert_eq!(counter.get(), 0);
    }

    #[test]
    fn test_gauge_metrics() {
        let gauge = Gauge::new();
        assert_eq!(gauge.get(), 0);

        gauge.set(100);
        assert_eq!(gauge.get(), 100);

        gauge.inc();
        assert_eq!(gauge.get(), 101);

        gauge.dec();
        assert_eq!(gauge.get(), 100);
    }

    #[test]
    fn test_histogram_metrics() {
        let histogram = Histogram::new();

        histogram.observe(0.05);
        histogram.observe(0.10);
        histogram.observe(0.25);
        histogram.observe(0.50);

        assert_eq!(histogram.count(), 4);
        assert!(histogram.average() > 0.0);

        let buckets = histogram.buckets();
        assert!(!buckets.is_empty());
    }

    #[test]
    fn test_histogram_duration() {
        let histogram = Histogram::latency();

        histogram.observe_duration(Duration::from_millis(5));
        histogram.observe_duration(Duration::from_millis(10));
        histogram.observe_duration(Duration::from_millis(15));

        assert_eq!(histogram.count(), 3);
    }

    #[test]
    fn test_metrics_collector() {
        let collector = MetricsCollector::default_config();

        let requests = collector.counter("http_requests_total");
        requests.inc();
        requests.inc();

        let connections = collector.gauge("active_connections");
        connections.set(42);

        let latency = collector.histogram("request_latency_seconds");
        latency.observe(0.05);

        let counters = collector.get_counters();
        assert_eq!(counters.get("http_requests_total"), Some(&2));

        let gauges = collector.get_gauges();
        assert_eq!(gauges.get("active_connections"), Some(&42));
    }

    #[test]
    fn test_same_metric_returned() {
        let collector = MetricsCollector::default_config();

        let c1 = collector.counter("test");
        let c2 = collector.counter("test");

        c1.inc();
        assert_eq!(c2.get(), 1);
    }

    #[test]
    fn test_prometheus_export() {
        let collector = MetricsCollector::default_config();

        collector.counter("requests").add(100);
        collector.gauge("connections").set(10);
        collector.histogram("latency").observe(0.05);

        let output = collector.export_text();

        assert!(output.contains("requests 100"));
        assert!(output.contains("connections 10"));
        assert!(output.contains("latency_count"));
        assert!(output.contains("latency_sum"));
    }

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
    fn test_liveness_check() {
        let health = HealthCheck::new();

        let result = health.liveness_check();
        assert_eq!(result.name, "liveness");
        assert_eq!(result.status, HealthStatus::Healthy);

        health.set_live(false);
        let result = health.liveness_check();
        assert_eq!(result.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_readiness_check() {
        let health = HealthCheck::new();

        let result = health.readiness_check();
        assert_eq!(result.name, "readiness");
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(result.message.is_some());

        health.set_ready(true);
        let result = health.readiness_check();
        assert_eq!(result.status, HealthStatus::Healthy);
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
    fn test_health_response_aggregation() {
        let checks = vec![
            HealthCheckResult::healthy("check1"),
            HealthCheckResult::healthy("check2"),
        ];
        let response = HealthResponse::new(checks, Duration::from_secs(60));
        assert_eq!(response.status, HealthStatus::Healthy);

        let checks = vec![
            HealthCheckResult::healthy("check1"),
            HealthCheckResult::degraded("check2", "slow"),
        ];
        let response = HealthResponse::new(checks, Duration::from_secs(60));
        assert_eq!(response.status, HealthStatus::Degraded);

        let checks = vec![
            HealthCheckResult::healthy("check1"),
            HealthCheckResult::unhealthy("check2", "failed"),
        ];
        let response = HealthResponse::new(checks, Duration::from_secs(60));
        assert_eq!(response.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_uptime_tracking() {
        let collector = MetricsCollector::default_config();
        std::thread::sleep(Duration::from_millis(10));
        assert!(collector.uptime() >= Duration::from_millis(10));

        let health = HealthCheck::new();
        std::thread::sleep(Duration::from_millis(10));
        assert!(health.uptime() >= Duration::from_millis(10));
    }

    #[test]
    fn test_health_serialization() {
        let result = HealthCheckResult::healthy("test")
            .with_duration(Duration::from_millis(5));

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("Healthy"));

        let deserialized: HealthCheckResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_observability_config() {
        let config = ObservabilityConfig::default();

        assert!(config.metrics_enabled);
        assert!(config.tracing_enabled);
        assert!(config.histograms_enabled);
        assert!(config.trace_sampling_rate > 0.0);
        assert!(config.collection_interval > Duration::ZERO);
    }

    #[test]
    fn test_metrics_reset() {
        let collector = MetricsCollector::default_config();

        collector.counter("test").add(100);
        assert_eq!(collector.counter("test").get(), 100);

        collector.reset();
        assert_eq!(collector.counter("test").get(), 0);
    }
}

// =============================================================================
// Cross-Feature Integration Tests
// =============================================================================

mod cross_feature_tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_metrics_integration() {
        use couchestor::rustfs::cache::{CacheEntry, CacheKey, CacheManager};

        let manager = CacheManager::in_memory();

        for i in 0..10 {
            let key = CacheKey::new("bucket", format!("key-{}", i));
            let entry = CacheEntry::new(Bytes::from(format!("data-{}", i)));
            manager.put(key.clone(), entry).await.unwrap();
            manager.get(&key).await;
        }

        let metrics = manager.metrics();

        assert!(metrics.l1_hits > 0);
        assert!(metrics.l1_size_bytes > 0);
        assert!(metrics.l1_entries > 0);
    }

    #[test]
    fn test_cross_component_health() {
        use couchestor::rustfs::monitoring::HealthCheck;

        let cache_health = HealthCheck::new();

        cache_health.set_ready(true);
        assert!(cache_health.is_ready());

        cache_health.set_ready(false);
        assert!(!cache_health.is_ready());
    }
}

// =============================================================================
// Edition Tests
// =============================================================================

mod edition_tests {
    #[test]
    fn test_edition_detection() {
        assert_eq!(couchestor::edition(), "Community");
    }

    #[test]
    fn test_edition_flags() {
        assert!(couchestor::is_community());
        assert!(!couchestor::is_enterprise());
    }

    #[test]
    fn test_enterprise_features_empty() {
        let features = couchestor::enterprise_features();
        assert!(features.is_empty(), "CE should have no enterprise features");
    }

    #[test]
    fn test_compression_algorithms() {
        use couchestor::rustfs::cache::compression::CompressionAlgorithm;

        let algs = CompressionAlgorithm::available_algorithms();
        assert!(algs.contains(&CompressionAlgorithm::None));
        assert!(algs.contains(&CompressionAlgorithm::Lz4));
        assert_eq!(algs.len(), 2, "CE only has None and LZ4");
    }
}
